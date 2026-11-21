"""Observed/report separation, compaction boundaries and post-exit flushes."""
import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from python_fakes import enable_python_fakes

from handoff_receipts import capture, query
from handoff_receipts.cli import main
from handoff_receipts.common import digest
from handoff_receipts.native import read_native
from handoff_receipts.diagnostics import ObservationError
from handoff_receipts.reported import summarize
from handoff_receipts.store import Store

FAKE = Path(__file__).with_name('fake_exec.py').resolve()


class Version011Test(unittest.TestCase):
    def setUp(self):
        enable_python_fakes(self)
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.sessions = self.root / 'sessions'
        self.store = Store(self.root / 'receipts')
        self.instruction = self.root / 'instruction.txt'
        self.instruction.write_text('Synthetic 0.1.1 instruction.\n')
        self.env = patch.dict(os.environ, {'FAKE_SESSIONS': str(self.sessions), 'FAKE_MODE': 'completed'})
        self.env.start()

    def tearDown(self):
        self.env.stop()
        self.tmp.cleanup()

    def run_case(self, mode, **kw):
        with patch.dict(os.environ, {'FAKE_MODE': mode}):
            aid = capture.prepare(self.store, self.instruction, codex=FAKE, cwd=self.root,
                                  session_root=self.sessions, observation_timeout=kw.pop('observation_timeout', 0), **kw)
            code = capture.execute(self.store, aid)
        return aid, code, query.inspect(self.store, aid)

    def test_compaction_preserves_exact_turn_binding_and_omits_private_context(self):
        aid, code, row = self.run_case('compacted')
        self.assertEqual(code, 0)
        self.assertTrue(row['facts']['target_input_confirmed'])
        self.assertEqual(row['facts']['turn_status'], 'completed')
        self.assertEqual(row['fact_levels']['turn_status'], 'observed')
        self.assertEqual(row['reported']['turn_id'], 'unknown')
        self.assertNotEqual(row['turn_id'], 'unknown')
        for path in self.store.path(aid).rglob('*'):
            if path.is_file():
                self.assertNotIn(b'COMPACTED_PRIVATE_CONTEXT', path.read_bytes())
                self.assertNotIn(b'private@example.invalid', path.read_bytes())
        segment = self.store.load(aid)[2]['target']['segments'][0]
        self.assertTrue(any(r['end'] - r['start'] > 1152000 for r in segment['omitted']))

    def test_compaction_cannot_connect_a_different_terminal_turn(self):
        _, _, row = self.run_case('compacted_wrong_turn')
        self.assertEqual(row['observed_facts']['turn_status'], 'unknown')
        self.assertEqual(row['fact_levels']['turn_status'], 'reported')
        self.assertIn('no_matching_turn', row['issues'])

    def test_missing_observation_keeps_reports_and_diagnostic_and_output(self):
        output = self.root / 'answer.md'
        aid, code, row = self.run_case('reported_only', output=output)
        self.assertEqual(code, 0)
        self.assertTrue(row['facts']['turn_started'])
        self.assertEqual(row['fact_levels']['turn_started'], 'reported')
        self.assertEqual(row['facts']['turn_status'], 'completed')
        self.assertEqual(row['fact_levels']['turn_status'], 'reported')
        self.assertEqual(row['observed_facts']['turn_status'], 'unknown')
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        self.assertEqual(row['identity_levels']['thread_id'], 'reported')
        self.assertEqual(row['identity_levels']['turn_id'], 'reported')
        self.assertEqual(row['observation_issues'][0]['code'], 'rollout_not_found')
        self.assertTrue(row['observation_issues'][0]['description'])
        self.assertEqual(output.read_text(), 'ACK')
        self.assertEqual(row['output']['assessment'], 'reported')
        self.assertEqual(row['output']['source'], 'exec_json_stream')
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)
        self.assertEqual(query.duplicates(self.store, aid)['effective_attempt'], 'unknown')
        self.assertEqual(query.which(self.store, str(self.instruction))['matches'], [])
        with contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(main(['--store', str(self.store.root), 'inspect', aid]), 0)
        self.assertIn('completed (reported)', out.getvalue())
        self.assertIn('rollout_not_found', out.getvalue())

    def test_cli_success_for_completed_report_and_warning(self):
        output = self.root / 'answer.md'
        with patch.dict(os.environ, {'FAKE_MODE': 'reported_only'}), contextlib.redirect_stdout(io.StringIO()) as out, contextlib.redirect_stderr(io.StringIO()) as err:
            code = main(['--store', str(self.store.root), 'run', str(self.instruction), '--codex', str(FAKE),
                         '--session-root', str(self.sessions), '-C', str(self.root), '--observation-timeout', '0',
                         '-o', str(output), '--json'])
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(out.getvalue())['fact_levels']['turn_status'], 'reported')
        self.assertIn('target_observation', err.getvalue())
        self.assertIn('rollout_not_found', err.getvalue())
        self.assertEqual(output.read_text(), 'ACK')

    def test_observed_terminal_and_output_take_precedence(self):
        output = self.root / 'answer.md'
        aid, _, row = self.run_case('completed', output=output)
        self.assertEqual(row['fact_levels']['turn_status'], 'observed')
        self.assertEqual(row['output']['assessment'], 'observed')
        state = self.store.load(aid)[2]
        message = next(e for e in state['control'] if e['event'] == 'stream_final_message')
        message.update(text='OTHER REPORT', sha256=digest('OTHER REPORT'))
        self.store.save(aid, state)
        capture.output_message(self.store, aid, state)
        self.assertEqual(output.read_text(), 'ACK')
        _, _, conflicting = self.run_case('stream_conflict')
        self.assertEqual(conflicting['reported']['turn_status'], 'completed')
        self.assertEqual(conflicting['facts']['turn_status'], 'failed')
        self.assertEqual(conflicting['fact_levels']['turn_status'], 'observed')

    def test_late_rollout_flush_after_process_exit_is_reread(self):
        aid, code, row = self.run_case('delayed', observation_timeout=2)
        self.assertEqual(code, 0)
        self.assertEqual(row['facts']['turn_status'], 'completed')
        self.assertEqual(row['fact_levels']['turn_status'], 'observed')
        self.assertEqual(row['issues'], [])
        tries = row['observation_attempts']
        self.assertGreater(len(tries), 1)
        self.assertIn('rollout_not_found', tries[0]['codes'])
        self.assertEqual(tries[-1]['codes'], [])
        control = self.store.load(aid)[2]['control']
        exited = next(e['at'] for e in control if e['event'] == 'process_exited')
        self.assertGreaterEqual(tries[0]['at'], exited)

    def test_recover_upgrades_report_without_mutating_request_and_honors_output_override(self):
        aid, _, before = self.run_case('reported_only', output=self.root / 'original.md')
        request_bytes = (self.store.path(aid) / 'request.json').read_bytes()
        tid, turn = before['thread_id'], before['turn_id']
        rows = [{'type': 'session_meta', 'payload': {'id': tid}},
                {'type': 'event_msg', 'payload': {'type': 'task_started', 'turn_id': turn}},
                {'type': 'response_item', 'payload': {'type': 'message', 'role': 'user', 'content': [
                    {'type': 'input_text', 'text': self.instruction.read_text()}]}},
                {'type': 'event_msg', 'timestamp': '2026-11-01T00:00:00Z', 'payload': {
                    'type': 'task_complete', 'turn_id': turn, 'last_agent_message': 'OBSERVED'}}]
        (self.sessions / ('rollout-' + tid + '.jsonl')).write_text(''.join(json.dumps(r) + '\n' for r in rows))
        destination = self.root / 'recovered.md'
        capture.recover(self.store, aid, output=destination)
        after = query.inspect(self.store, aid)
        self.assertEqual(after['fact_levels']['turn_status'], 'observed')
        self.assertEqual(after['output']['assessment'], 'observed')
        self.assertEqual(destination.read_text(), 'OBSERVED')
        self.assertEqual((self.root / 'original.md').read_text(), 'ACK')
        self.assertEqual((self.store.path(aid) / 'request.json').read_bytes(), request_bytes)

    def test_sensitive_stream_final_is_not_saved_or_written(self):
        output = self.root / 'answer.md'
        aid, _, row = self.run_case('reported_sensitive', output=output)
        self.assertFalse(output.exists())
        self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
        self.assertEqual(row['output']['status'], 'not_written')
        for p in self.store.path(aid).rglob('*'):
            if p.is_file():
                self.assertNotIn(b'private@example.invalid', p.read_bytes())

    def test_native_errors_have_specific_safe_codes(self):
        path = self.root / 'rollout.jsonl'
        for text, code in [('', 'rollout_incomplete'), ('{bad}\n', 'rollout_invalid_json'),
                           ('{}\n', 'rollout_metadata_missing'),
                           (json.dumps({'type': 'session_meta', 'payload': {'id': 'other'}}) + '\n', 'rollout_thread_mismatch')]:
            with self.subTest(code=code):
                path.write_text(text)
                with self.assertRaises(ObservationError) as error:
                    read_native(path, 'thread')
                self.assertEqual(error.exception.diagnostic['code'], code)
        aid, _, _ = self.run_case('completed')
        state = self.store.load(aid)[2]
        native = Path(state['target']['native_path'])
        native.write_text('{"private": "private@example.invalid"}broken\n')
        capture.recover(self.store, aid)
        row = query.inspect(self.store, aid)
        self.assertEqual(row['observation_issues'][0]['code'], 'rollout_invalid_json')
        self.assertNotIn('private@example.invalid', json.dumps(row['observation_issues']))

    def test_oversized_stream_line_does_not_reuse_earlier_final_message(self):
        output = self.root / 'answer.md'
        aid, code, row = self.run_case('reported_oversized', output=output)
        self.assertEqual(code, 0)
        self.assertEqual(row['facts']['turn_status'], 'completed')
        self.assertEqual(row['fact_levels']['turn_status'], 'reported')
        self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
        self.assertFalse(output.exists())
        self.assertIn('stream_record_too_large', [e['event'] for e in self.store.load(aid)[2]['control']])

    def test_ambiguous_or_mismatched_stream_cannot_supply_completion(self):
        request = {'via': 'exec', 'thread_id': 'thread'}
        base = [{'event': 'stream_thread', 'thread_id': 'thread'},
                {'event': 'stream_turn_started', 'turn_id': 'one'},
                {'event': 'stream_turn_completed', 'turn_id': 'two'}]
        self.assertEqual(summarize(request, base)['turn_status'], 'unknown')
        base[-1]['turn_id'] = 'one'
        base.append({'event': 'stream_turn_completed', 'turn_id': 'one'})
        self.assertEqual(summarize(request, base)['turn_status'], 'unknown')


if __name__ == '__main__':
    unittest.main()
