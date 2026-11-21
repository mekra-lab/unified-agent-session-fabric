import contextlib
import io
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch
from python_fakes import cli_args, enable_python_fakes
from handoff_receipts import capture, export, query
from handoff_receipts.cli import main
from handoff_receipts.common import read_json
from handoff_receipts.store import Store
from handoff_receipts.rpc import Rpc
from fake_daemon import FakeDaemon

FAKE = Path(__file__).with_name('fake_exec.py').resolve()
SRC = Path(__file__).resolve().parents[1] / 'src'


class ReceiptsTest(unittest.TestCase):
    def setUp(self):
        enable_python_fakes(self)
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.sessions = self.root / 'sessions'
        self.sessions.mkdir()
        self.instruction = self.root / 'instruction.txt'
        self.instruction.write_text('Reply ACK.\n')
        self.store = Store(self.root / 'store')
        self.env = patch.dict(os.environ, {'FAKE_SESSIONS': str(self.sessions), 'FAKE_MODE': 'completed'})
        self.env.start()

    def tearDown(self):
        self.env.stop()
        self.temp.cleanup()

    def prepare(self, **kw):
        kw.setdefault('observation_timeout', 0)
        return capture.prepare(self.store, self.instruction, codex=FAKE,
                               session_root=self.sessions, cwd=self.root, **kw)

    def run_case(self, mode='completed', **kw):
        with patch.dict(os.environ, {'FAKE_MODE': mode}):
            aid = self.prepare(**kw)
            code = capture.execute(self.store, aid)
        return aid, code, query.inspect(self.store, aid)

    def test_normal_independent_facts_and_determinism(self):
        aid, code, row = self.run_case()
        self.assertEqual(code, 0)
        facts = row['facts']
        self.assertTrue(facts['process_started'])
        self.assertTrue(facts['target_input_confirmed'])
        self.assertTrue(facts['turn_started'])
        self.assertEqual(facts['turn_status'], 'completed')
        self.assertNotEqual(facts['completed_at'], 'unknown')
        self.assertEqual(facts['model_read_or_used'], 'unknown')
        self.assertEqual(row, query.inspect(self.store, aid))

    def test_failed(self):
        aid, code, row = self.run_case('failed')
        self.assertEqual(code, 1)
        self.assertEqual(row['facts']['turn_status'], 'failed')
        self.assertTrue(row['facts']['target_input_confirmed'])

    def test_task_complete_with_error_is_failure(self):
        _, code, row = self.run_case('failed_complete')
        self.assertEqual(code, 1)
        self.assertEqual(row['facts']['turn_status'], 'failed')
        self.assertEqual(row['facts']['completed_at'], 'unknown')

    def test_process_failure_not_turn_failure(self):
        aid = self.prepare()
        (self.store.path(aid) / 'request.json')  # immutable request is not modified
        with patch('handoff_receipts.capture.subprocess.Popen', side_effect=FileNotFoundError):
            self.assertEqual(capture.execute(self.store, aid), 127)
        self.assertEqual(query.inspect(self.store, aid)['facts']['turn_status'], 'unknown')

    def test_loss_without_stream_unique_native_recovery(self):
        _, code, row = self.run_case('loss')
        self.assertEqual(code, 0)
        self.assertEqual(row['facts']['turn_status'], 'completed')
        self.assertNotEqual(row['thread_id'], 'unknown')

    def test_missing_observation_exit_zero_not_completion(self):
        _, code, row = self.run_case('missing')
        self.assertEqual(code, 0)
        self.assertEqual(row['facts']['process_exit_code'], 0)
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        self.assertEqual(query.duplicates(self.store, row['attempt_id'])['effective_attempt'], 'unknown')

    def test_explicit_retry_and_resume(self):
        first, _, row = self.run_case()
        second, _, last = self.run_case(resume=row['thread_id'], retry_of=first)
        self.assertNotEqual(row['turn_id'], last['turn_id'])
        dup = query.duplicates(self.store, second)
        self.assertEqual(dup['duplicate_execution'], True)
        self.assertEqual(dup['effective_attempt'], first)
        self.instruction.write_text('Changed body')
        with self.assertRaisesRegex(ValueError, 'identical'):
            self.prepare(retry_of=first)

    def test_resume_preflight_prefix_change_is_unknown(self):
        _, _, first = self.run_case()
        aid = self.prepare(resume=first['thread_id'])
        native = next(self.sessions.glob('*.jsonl'))
        text = native.read_text().replace('SYNTHETIC_PRIVATE_ACCOUNT', 'OTHER_SYNTHETIC_ACCOUNT')
        native.write_text(text)
        capture.execute(self.store, aid)
        row = query.inspect(self.store, aid)
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        self.assertEqual(row['facts']['turn_status'], 'completed')
        self.assertEqual(row['fact_levels']['turn_status'], 'reported')
        self.assertEqual(row['observed_facts']['turn_status'], 'unknown')
        self.assertTrue(row['issues'])

    def test_privacy_selected_lines(self):
        aid, _, row = self.run_case()
        for file in self.store.path(aid).rglob('*'):
            if file.is_file():
                raw = file.read_bytes()
                for sentinel in (b'private@example.invalid', b'OMITTED_PRIVATE', b'SYNTHETIC_PRIVATE_ACCOUNT'):
                    self.assertNotIn(sentinel, raw)
        seg = self.store.load(aid)[2]['target']['segments'][0]
        self.assertTrue(seg['omitted'])
        self.assertTrue(seg['lines'])
        self.assertTrue(row['facts']['target_input_confirmed'])
        self.assertNotEqual(row['facts']['completed_at'], 'unknown')

    def test_sensitive_required_line_unknown(self):
        _, _, row = self.run_case('sensitive_input')
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        self.assertEqual(row['facts']['turn_status'], 'completed')
        self.assertEqual(query.which(self.store, str(self.instruction))['matches'], [])

    def test_known_turn_without_input_or_start_preserves_terminal(self):
        for mode in ('missing_input', 'missing_start'):
            with self.subTest(mode=mode):
                _, _, row = self.run_case(mode)
                self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
                self.assertEqual(row['facts']['turn_status'], 'completed')
                self.assertEqual(row['observed_facts']['turn_started'], 'unknown' if mode == 'missing_start' else True)
                self.assertTrue(row['facts']['turn_started'])

    def test_sensitive_final_omits_only_affected_facts(self):
        output = self.root / 'answer.md'
        _, _, row = self.run_case('sensitive_final', output=output)
        self.assertTrue(row['facts']['target_input_confirmed'])
        self.assertEqual(row['facts']['turn_status'], 'completed')
        self.assertEqual(row['fact_levels']['turn_status'], 'reported')
        self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
        self.assertFalse(output.exists())

    def test_sensitive_instruction_refused_before_store_or_process(self):
        # Synthetic detector input, assembled at runtime so the release scanner does not flag it.
        self.instruction.write_text('value ' + 'sk-' + 'synthetic-secret')
        with self.assertRaisesRegex(ValueError, 'sensitive'):
            self.prepare()
        self.assertFalse(self.store.root.exists())

    def test_permissions_output_forget(self):
        output = self.root / 'answer.md'
        aid, _, _ = self.run_case(output=output)
        self.assertEqual(output.read_text(), 'ACK')
        for file in [self.store.root, *self.store.root.rglob('*'), output]:
            self.assertEqual(file.stat().st_mode & 0o777, 0o700 if file.is_dir() else 0o600)
        self.store.forget(aid)
        self.assertEqual(self.store.ids(), [])
        self.assertTrue(output.exists())
        self.assertTrue(list(self.sessions.glob('*.jsonl')))

    def test_core_tamper(self):
        aid, _, _ = self.run_case()
        (self.store.path(aid) / 'body.txt').write_text('tampered')
        with self.assertRaisesRegex(ValueError, 'integrity'):
            query.inspect(self.store, aid)
        with self.assertRaises(ValueError):
            capture.recover(self.store, aid)

    def test_checkpoint_tamper(self):
        aid, _, _ = self.run_case()
        folder = self.store.path(aid)
        head = (folder / 'CURRENT').read_text().strip()
        (folder / 'states' / (head + '.json')).write_text('{}')
        with self.assertRaisesRegex(ValueError, 'integrity'):
            query.inspect(self.store, aid)

    def test_offline_queries_after_originals_removed(self):
        aid, _, row = self.run_case()
        self.instruction.unlink()
        for path in self.sessions.glob('*.jsonl'):
            path.unlink()
        with patch('handoff_receipts.capture.read_native', side_effect=AssertionError('native access')):
            self.assertEqual(query.inspect(self.store, aid), row)
            self.assertEqual(len(query.which(self.store, row['source_path'])['matches']), 1)
            self.assertEqual(query.duplicates(self.store, aid)['effective_attempt'], aid)
            output = self.root / 'export.jsonl'
            export.jsonl(self.store, [aid], output)
            self.assertEqual(json.loads(output.read_text())['body'], 'Reply ACK.\n')

    def test_polylogue_missing_bridge(self):
        aid, _, _ = self.run_case()
        with self.assertRaisesRegex(ValueError, 'Rust bridge'):
            export.polylogue(self.store, [aid], self.root / 'export', None)

    def test_refuses_redirection_passthrough_and_symlinks(self):
        for args in (['--ephemeral'], ['-o', 'out'], ['--last']):
            with self.assertRaises(ValueError):
                self.prepare(extra=args)
        real = self.root / 'real'
        real.mkdir()
        link = self.root / 'link'
        link.symlink_to(real)
        with self.assertRaises(ValueError):
            Store(link)

    def test_command_resume_and_wrapper_output(self):
        args = capture.command('codex', 'thread-1', 'model', 'low', '/cwd', ['-c', 'sandbox_mode=read-only'])
        self.assertEqual(args[:3], ['codex', 'exec', 'resume'])
        self.assertNotIn('-C', args)
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            code = main(['--store', str(self.store.root), 'run', str(self.instruction), '--codex', str(FAKE),
                         '--session-root', str(self.sessions), '-C', str(self.root), '-o', str(self.root / 'out'), '-n', 'review', '--json'])
        self.assertEqual(code, 0)
        self.assertEqual((self.root / 'out/review.md').read_text(), 'ACK')

    def test_cli_query_verify_export_forget(self):
        aid, _, row = self.run_case()
        for command in (['inspect', aid], ['dup', aid], ['which', row['instruction_sha256']], ['list'],
                        ['verify', aid], ['export', aid, '--format', 'jsonl', '--output', str(self.root / 'out.jsonl')]):
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                self.assertEqual(main(['--store', str(self.store.root), *command, '--json']), 0)
            self.assertIsNotNone(json.loads(out.getvalue()))
        with contextlib.redirect_stdout(io.StringIO()) as human:
            self.assertEqual(main(['--store', str(self.store.root), 'inspect', aid]), 0)
        self.assertIn('target input: True', human.getvalue())
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(main(['--store', str(self.store.root), 'forget', aid, '--json']), 0)
        self.assertEqual(self.store.ids(), [])

    def start_waiting(self):
        env = {**os.environ, 'PYTHONPATH': str(SRC), 'FAKE_MODE': 'wait'}
        proc = subprocess.Popen(cli_args('--store', str(self.store.root),
            'run', str(self.instruction), '--codex', str(FAKE), '--session-root', str(self.sessions),
            '-C', str(self.root), '--json'), env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        deadline = time.monotonic() + 10
        aid = None
        while time.monotonic() < deadline:
            ids = self.store.ids()
            if ids:
                try:
                    state = self.store.load(ids[0])[2]
                    if state['target'].get('turn_id'):
                        aid = ids[0]
                        break
                except (FileNotFoundError, KeyError):
                    pass
            time.sleep(.02)
        self.assertIsNotNone(aid)
        return proc, aid

    def test_sigint_forwarding(self):
        proc, aid = self.start_waiting()
        proc.send_signal(signal.SIGINT)
        proc.communicate(timeout=10)
        row = query.inspect(self.store, aid)
        self.assertEqual(row['facts']['turn_status'], 'interrupted')
        self.assertEqual(row['facts']['process_exit_code'], 130)

    def test_sigkill_then_recover_no_resend(self):
        proc, aid = self.start_waiting()
        state = self.store.load(aid)[2]
        child = next(e['pid'] for e in state['control'] if e['event'] == 'process_started')
        proc.kill()
        proc.wait(timeout=10)
        # Kill the dedicated native group too: abrupt death may have no terminal.
        os.killpg(child, signal.SIGKILL)
        proc.communicate(timeout=10)
        before = len(list(self.sessions.glob('*.jsonl')))
        capture.recover(self.store, aid)
        row = query.inspect(self.store, aid)
        self.assertTrue(row['facts']['target_input_confirmed'])
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertEqual(row['facts']['process_exit_code'], 'unknown')
        self.assertEqual(len(list(self.sessions.glob('*.jsonl'))), before)

    def test_recovery_of_later_completion_and_live_lock(self):
        proc, aid = self.start_waiting()
        with self.assertRaisesRegex(ValueError, 'still being'):
            capture.recover(self.store, aid)
        state = self.store.load(aid)[2]
        child = next(e['pid'] for e in state['control'] if e['event'] == 'process_started')
        proc.kill()
        proc.wait(timeout=10)
        os.killpg(child, signal.SIGINT)
        proc.communicate(timeout=10)
        capture.recover(self.store, aid)
        self.assertEqual(query.inspect(self.store, aid)['facts']['turn_status'], 'interrupted')

    def test_fake_daemon_exact_id(self):
        with FakeDaemon(self.sessions, self.root / 'socket') as fake:
            client = Rpc(self.root / 'socket')
            thread = client.call('thread/start', {'cwd': str(self.root)})['thread']['id']
            client.close()
            fake.unmaterialized = 1
            aid = capture.prepare(self.store, self.instruction, via='daemon', thread=thread,
                                  socket=self.root / 'socket', cwd=self.root, session_root=self.sessions)
            self.assertEqual(capture.execute(self.store, aid), 0)
            row = query.inspect(self.store, aid)
            self.assertEqual(row['facts']['turn_status'], 'completed')
            self.assertEqual(row['facts']['process_started'], 'unknown')
            self.assertTrue(row['facts']['target_input_confirmed'])
            self.assertEqual(sum(r['method'] == 'turn/start' for r in fake.calls), 1)

    def test_daemon_response_loss_observes_without_resend(self):
        with FakeDaemon(self.sessions, self.root / 'socket') as fake:
            client = Rpc(self.root / 'socket')
            thread = client.call('thread/start', {'cwd': str(self.root)})['thread']['id']
            fake.drop_start_response = True
            aid = capture.prepare(self.store, self.instruction, via='daemon', thread=thread,
                                  socket=self.root / 'socket', cwd=self.root, session_root=self.sessions)
            capture.execute(self.store, aid)
            self.assertEqual(query.inspect(self.store, aid)['facts']['turn_status'], 'completed')
            self.assertEqual(sum(r['method'] == 'turn/start' for r in fake.calls), 1)
            self.assertEqual(len(client.call('thread/turns/list', {'threadId': thread})['data']), 1)
            client.close()

    def test_unexpected_file_and_selected_copy_tamper(self):
        aid, _, _ = self.run_case()
        unexpected = self.store.path(aid) / 'extra.txt'
        unexpected.write_text('unexpected')
        with self.assertRaisesRegex(ValueError, 'unexpected'):
            self.store.load(aid)
        unexpected.unlink()
        state = self.store.load(aid)[2]
        state['target']['segments'][0]['copy_text'] += 'changed'
        self.store.save(aid, state)
        with self.assertRaisesRegex(ValueError, 'copy hash'):
            query.inspect(self.store, aid)

    def test_r3_relative_path_and_default_store(self):
        _, _, row = self.run_case()
        from handoff_receipts.store import default_root
        with patch.dict(os.environ, {'XDG_DATA_HOME': str(self.root)}):
            self.assertEqual(default_root(), self.root / 'handoff-receipts')
        self.assertEqual(query.which(self.store, row['instruction_sha256'])['matches'][0], row)
        self.assertEqual(query.which(self.store, 'f' * 64)['outside_observation'], 'unknown')

    def test_daemon_busy_rejected(self):
        with FakeDaemon(self.sessions, self.root / 'socket') as fake:
            client = Rpc(self.root / 'socket')
            thread = client.call('thread/start', {'cwd': str(self.root)})['thread']['id']
            client.close()
            fake.threads[thread]['status'] = {'type': 'active'}
            aid = capture.prepare(self.store, self.instruction, via='daemon', thread=thread,
                                  socket=self.root / 'socket', cwd=self.root, session_root=self.sessions)
            self.assertEqual(capture.execute(self.store, aid), 2)
            self.assertFalse(any(r['method'] == 'turn/start' for r in fake.calls))


if __name__ == '__main__':
    unittest.main()
