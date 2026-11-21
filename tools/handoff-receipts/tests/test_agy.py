"""AG1 contract cases, entirely synthetic; never execute an installed agy."""
import base64
import contextlib
import io
import json
import os
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch
from python_fakes import cli_args, enable_python_fakes

from handoff_receipts import agy, capture, export, query
from handoff_receipts.cli import main, parser
from handoff_receipts.common import canonical, digest
from handoff_receipts.diagnostics import ObservationError
from handoff_receipts.protowire import fields, nested, one
from handoff_receipts.store import Store

FAKE = Path(__file__).with_name('fake_agy.py').resolve()
FIXTURE = Path(__file__).with_name('fixtures') / 'agy-wire.json'
PRODUCTION_VERIFIED = dict(agy.VERIFIED)


class AG1Test(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        enable_python_fakes(self, [self.root / 'fake_agy_copy.py',
                                  self.root / 'child/bin/fake-agy',
                                  self.root / 'wrapper/bin/fake-agy'])
        self.native = self.root / 'conversations'
        self.store = Store(self.root / 'store')
        self.prompt = self.root / 'input.txt'
        self.prompt.write_text(' \nSynthetic AG1 café \ud55c\uae00 e\u0301 🧪\t\n ')
        self.trace = self.root / 'calls.jsonl'
        self.env = patch.dict(os.environ, {'FAKE_AGY_ROOT': str(self.native), 'FAKE_AGY_MODE': 'normal',
                                          'FAKE_AGY_VERSION': '1.5.9', 'FAKE_AGY_TRACE': str(self.trace)})
        self.env.start()
        # Trust only the synthetic producer for synthetic interpretation tests.
        # Production constants have no test or environment bypass.
        self.version = patch.object(agy, 'VERIFIED', {'version': '1.5.9', 'binary_sha256': digest(FAKE.read_bytes())})
        self.version.start()

    def tearDown(self):
        self.version.stop()
        self.env.stop()
        self.tmp.cleanup()

    def prepare(self, **kw):
        kw.setdefault('observation_timeout', 0)
        return capture.prepare(self.store, self.prompt, engine='agy', agy=FAKE, cwd=self.root,
                               session_root=self.native, **kw)

    def run_case(self, mode='normal', **kw):
        with patch.dict(os.environ, {'FAKE_AGY_MODE': mode}):
            aid = self.prepare(**kw)
            code = capture.execute(self.store, aid)
        return aid, code, query.inspect(self.store, aid)

    def test_r1_recover_pins_input_after_same_body_resume(self):
        first, _, original = self.run_case()
        _, _, resumed = self.run_case(resume=original['conversation_id'])
        self.assertNotEqual(original['turn_id'], resumed['turn_id'])
        capture.recover(self.store, first)
        recovered = query.inspect(self.store, first)
        self.assertNotIn('ambiguous_target_match', recovered['issues'])
        self.assertIs(recovered['facts']['target_input_confirmed'], True)
        self.assertEqual(recovered['turn_id'], original['turn_id'])
        self.assertEqual(recovered['auxiliary'], original['auxiliary'])

    def test_r2_recover_preserves_stream_report_and_output(self):
        for mode in ('normal', 'null_reason'):
            with self.subTest(mode=mode):
                answer = self.root / (mode + '.md')
                aid, _, original = self.run_case(mode, output=answer)
                controls = [e for e in self.store.load(aid)[2]['control'] if e['event'].startswith('agy_')]
                # A failed native reread must leave the child's saved report usable.
                with patch.object(agy, 'read_database', side_effect=ObservationError('agy_native_incomplete')):
                    capture.recover(self.store, aid, observation_timeout=0)
                recovered = query.inspect(self.store, aid)
                self.assertEqual(recovered['facts']['turn_status'], 'completed')
                self.assertEqual(recovered['facts']['final_output_sha256'], digest('AGY-ACK\n'))
                self.assertEqual(recovered['reported'], original['reported'])
                self.assertEqual(answer.read_bytes(), b'AGY-ACK\n')
                current = [e for e in self.store.load(aid)[2]['control']
                           if e['event'].startswith('agy_') and e['event'] != 'agy_version_recover']
                self.assertEqual(current, controls)

    def test_b1_recover_requires_unchanged_executable_stat(self):
        stat = FAKE.stat()
        names = ('st_dev', 'st_ino', 'st_size', 'st_mtime_ns', 'st_ctime_ns')
        identity = {name: getattr(stat, name) for name in names}
        before = {'event': 'agy_version_before', **agy.VERIFIED, 'stat': identity}
        recovered = {'event': 'agy_version_recover', **agy.VERIFIED, 'stat': dict(identity)}
        self.assertTrue(agy.version_verified([before, recovered]))
        for name in names:
            with self.subTest(field=name):
                changed = {**recovered, 'stat': {**identity, name: identity[name] + 1}}
                self.assertFalse(agy.version_verified([before, changed]))
        for missing in ({**recovered, 'stat': {}}, {k: v for k, v in recovered.items() if k != 'stat'}):
            with self.subTest(missing=missing):
                self.assertFalse(agy.version_verified([before, missing]))
        request = {'argv': [str(FAKE)], 'cwd': str(self.root)}
        self.assertEqual(agy.fingerprint(request)['stat'], identity)
        self.assertFalse(agy.version_verified([{k: v for k, v in before.items() if k != 'stat'}, recovered]))
        after = {'event': 'agy_version_after', **agy.VERIFIED}
        self.assertTrue(agy.version_verified([{k: v for k, v in before.items() if k != 'stat'}, after]))

        executable = self.root / 'restored_fake.py'
        shutil.copy2(FAKE, executable)
        saved = executable.stat()
        request['argv'] = [str(executable)]
        initial = {'event': 'agy_version_before', **agy.fingerprint(request)}
        original = executable.read_bytes()
        executable.write_bytes(original + b'\n# transient change\n')
        executable.write_bytes(original)
        os.utime(executable, ns=(saved.st_atime_ns, saved.st_mtime_ns))
        latest = {'event': 'agy_version_recover', **agy.fingerprint(request)}
        self.assertEqual(initial['binary_sha256'], latest['binary_sha256'])
        self.assertEqual(initial['stat']['st_mtime_ns'], latest['stat']['st_mtime_ns'])
        self.assertNotEqual(initial['stat']['st_ctime_ns'], latest['stat']['st_ctime_ns'])
        self.assertFalse(agy.version_verified([initial, latest]))

    def test_b2_status_reason_matrix(self):
        sid = '00000000-0000-4000-8000-000000000001'
        absent = object()
        reasons = (absent, None, 'print_timeout', 'interrupted', 'future_reason', '', 3)
        for status in ('SUCCESS', 'ERROR', 'FUTURE_STATUS'):
            for reason in reasons:
                with self.subTest(status=status, reason=reason):
                    payload = {'conversation_id': sid, 'status': status, 'response': 'ACK'}
                    if reason is not absent:
                        payload['finished_reason'] = reason
                    controls = [{'event': name, **value} for name, value in agy.stream_events({'event': 'result', 'result': payload})]
                    expected = 'unknown'
                    if status == 'SUCCESS':
                        if reason is absent or reason is None:
                            expected = 'completed'
                        elif reason == 'print_timeout':
                            expected = 'failed'
                    elif status == 'ERROR':
                        expected = 'interrupted' if reason == 'interrupted' else 'failed'
                    self.assertEqual(agy.reported({'thread_id': None}, controls)['turn_status'], expected)
                    if reason is absent or reason is None:
                        self.assertIsNone(controls[-1]['finished_reason'])
                    elif reason == 'future_reason':
                        self.assertIsNotNone(controls[-1]['finished_reason'])

    def test_b3_incomplete_input_stream_blocks_capture_and_recover(self):
        for mode in ('missing_input_id', 'invalid_index_after', 'stream_incomplete'):
            with self.subTest(mode=mode):
                aid, _, row = self.run_case(mode)
                self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
                self.assertEqual(row['turn_id'], 'unknown')
                capture.recover(self.store, aid)
                self.assertEqual(query.inspect(self.store, aid)['facts']['target_input_confirmed'], 'unknown')

    def test_e3_only_success_requires_a_string_response(self):
        sid = '00000000-0000-4000-8000-000000000001'
        absent = object()
        for status in ('SUCCESS', 'ERROR', 'FUTURE_STATUS'):
            for reason in (absent, None, 'interrupted', 'print_timeout', 'future_reason'):
                for response in (absent, None, 3, {}, []):
                    with self.subTest(status=status, reason=reason, response=response):
                        payload = {'conversation_id': sid, 'status': status}
                        if reason is not absent:
                            payload['finished_reason'] = reason
                        if response is not absent:
                            payload['response'] = response
                        control = [{'event': name, **fields} for name, fields in
                                   agy.stream_events({'event': 'result', 'result': payload})]
                        report = agy.reported({'thread_id': None}, control)
                        expected = 'unknown'
                        if status == 'ERROR':
                            expected = 'interrupted' if reason == 'interrupted' else 'failed'
                        self.assertEqual(report['turn_status'], expected)
                        self.assertEqual(bool(control[-1].get('malformed')), status == 'SUCCESS')
                        self.assertIsNone(report['final_message'])
                        self.assertEqual(report['final_output_sha256'], 'unknown')

    def test_b4_omitted_planner_does_not_stop_executor_rereads(self):
        _, code, row = self.run_case('sensitive_late_cancel', observation_timeout=2)
        if code != 2:
            time.sleep(.5)  # Finish the synthetic writer before a failing baseline cleanup.
        self.assertEqual(code, 2)
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertIn('agy_status_conflict', row['issues'])
        self.assertIn('agy_fields_omitted', row['issues'])
        self.assertTrue(any({'agy_fields_omitted', 'agy_executor_pending'} <= set(a['codes'])
                            for a in row['observation_attempts']))
        self.assertGreater(len(row['observation_attempts']), 1)

    def test_b5_invalid_utf8_success_is_malformed(self):
        aid, code, row = self.run_case('invalid_utf8', output=self.root / 'invalid.md')
        self.assertEqual(code, 2)
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
        result = next(e for e in self.store.load(aid)[2]['control'] if e['event'] == 'agy_result')
        self.assertTrue(result['malformed'])
        self.assertFalse((self.root / 'invalid.md').exists())

    def test_b6_relative_path_uses_child_cwd_for_every_execution_check(self):
        wrapper = self.root / 'wrapper'
        child = self.root / 'child'
        for directory in (wrapper, child):
            (directory / 'bin').mkdir(parents=True)
            shutil.copy2(FAKE, directory / 'bin/fake-agy')
            # PATH lookup still requires an executable temporary fixture.
            (directory / 'bin/fake-agy').chmod(0o755)
        (wrapper / 'bin/fake-agy').write_bytes(FAKE.read_bytes() + b'\n# different wrapper executable\n')
        previous = Path.cwd()
        try:
            os.chdir(wrapper)
            with patch.dict(os.environ, {'PATH': 'bin' + os.pathsep + os.defpath}):
                for executable in ('fake-agy', 'bin/fake-agy'):
                    with self.subTest(executable=executable):
                        aid = capture.prepare(self.store, self.prompt, engine='agy', agy=executable,
                                              cwd=child, session_root=self.native, observation_timeout=0)
                        request = self.store.load(aid)[1]
                        self.assertEqual(request['argv'][0], str(child / 'bin/fake-agy'))
                        self.assertEqual(capture.execute(self.store, aid), 0)
                        row = query.inspect(self.store, aid)
                        self.assertTrue(row['version_verified'])
                        self.assertIs(row['facts']['target_input_confirmed'], True)
                        self.assertTrue(all(e['binary_sha256'] == digest((child / 'bin/fake-agy').read_bytes())
                                            for e in row['version_observations']))
        finally:
            os.chdir(previous)

    def test_normal_exact_utf8_input_and_reported_completion(self):
        aid = self.prepare(model='synthetic-model', effort='low', output=self.root / 'answer.md')
        source, request, _, _ = self.store.load(aid)
        self.assertIsNone(request['thread_id'])
        self.assertIsNone(request['preflight'])
        self.assertEqual(source['interpretation_profile'], agy.PROFILE)
        self.assertEqual(request['argv'][1:5], ['--input-format', 'stream-json', '--output-format', 'stream-json'])
        self.assertNotIn('-p', request['argv'])
        self.assertIn('--effort', request['argv'])
        before = (self.store.path(aid) / 'request.json').read_bytes()
        self.assertEqual(capture.execute(self.store, aid), 0)
        row = query.inspect(self.store, aid)
        self.assertEqual(row['engine_status'], 'experimental')
        self.assertTrue(row['version_verified'])
        self.assertIs(row['facts']['target_input_confirmed'], True)
        self.assertEqual(row['facts']['turn_status'], 'completed')
        self.assertEqual(row['fact_levels']['turn_status'], 'reported')
        self.assertEqual(row['fact_levels']['target_input_confirmed'], 'observed')
        self.assertEqual(row['facts']['final_output_sha256'], digest('AGY-ACK\n'))
        self.assertEqual(row['observed_facts']['turn_status'], 'unknown')
        for key in ('turn_started', 'completed_at', 'model_read_or_used'):
            self.assertEqual(row['facts'][key], 'unknown')
        self.assertEqual(row['turn_key'], {'kind': 'execution', 'value': row['turn_id']})
        self.assertNotEqual(row['conversation_id'], row['trajectory_id'])
        self.assertEqual(row['auxiliary']['termination_reasons'], [{'value': 4, 'name': 'NO_TOOL_CALL'}])
        self.assertEqual(row['output']['source'], 'agy_json_result')
        self.assertEqual((self.root / 'answer.md').read_bytes(), b'AGY-ACK\n')
        self.assertEqual(before, (self.store.path(aid) / 'request.json').read_bytes())
        self.assertEqual(len(self.trace.read_text().splitlines()), 1)

    def test_same_body_resume_and_full_execution_identity(self):
        first, _, before = self.run_case()
        second, code, after = self.run_case(resume=before['conversation_id'], retry_of=first)
        self.assertEqual(code, 0)
        self.assertIs(after['facts']['target_input_confirmed'], True)
        self.assertEqual(after['trajectory_id'], before['trajectory_id'])
        self.assertNotEqual(after['turn_id'], before['turn_id'])
        request = self.store.load(second)[1]
        self.assertEqual(request['preflight']['max_idx'], 1)
        self.assertEqual(len(request['preflight']['boundary_sha256']), 64)
        self.assertIn('--conversation', request['argv'])
        dup = query.duplicates(self.store, 'agy:' + first)
        self.assertTrue(dup['duplicate_execution'])
        self.assertEqual(dup['effective_attempt'], 'unknown')
        self.assertEqual(query.lookup(self.store, 'agy:' + after['turn_id'])[0]['attempt_id'], second)
        self.assertEqual(len(query.which(self.store, 'agy:' + str(self.prompt))['matches']), 2)

    def test_missing_resume_refused_before_any_process_or_send(self):
        with patch('subprocess.Popen', side_effect=AssertionError('must not launch')):
            with self.assertRaisesRegex(ValueError, 'no dispatch'):
                self.prepare(resume='00000000-0000-4000-8000-000000000001')
        self.assertFalse(self.trace.exists())
        self.assertEqual(self.store.ids(), [])

    def test_resume_meta_and_unreadable_boundary_refused(self):
        _, _, row = self.run_case()
        path = self.native / (row['conversation_id'] + '.db')
        with contextlib.closing(sqlite3.connect(path)) as c, c:
            c.execute('UPDATE trajectory_meta SET cascade_id=?', ('00000000-0000-4000-8000-000000000001',))
        with self.assertRaisesRegex(ValueError, 'no dispatch'):
            self.prepare(resume=row['conversation_id'])
        path.write_bytes(b'incomplete SQLite')
        with self.assertRaisesRegex(ValueError, 'no dispatch'):
            self.prepare(resume=row['conversation_id'])
        self.assertEqual(len(self.trace.read_text().splitlines()), 1)

    def test_resume_boundary_change_before_and_after_dispatch(self):
        _, _, row = self.run_case()
        aid = self.prepare(resume=row['conversation_id'])
        path = self.native / (row['conversation_id'] + '.db')
        with contextlib.closing(sqlite3.connect(path)) as c, c:
            c.execute('UPDATE steps SET step_payload=? WHERE idx=1', (b'changed',))
        self.assertEqual(capture.execute(self.store, aid), 2)
        self.assertFalse(query.inspect(self.store, aid)['facts']['dispatched'])
        self.assertEqual(len(self.trace.read_text().splitlines()), 1)
        # A separately captured resume loses native attribution if its saved boundary changes later.
        _, _, new = self.run_case()
        attempt, _, _ = self.run_case(resume=new['conversation_id'])
        with contextlib.closing(sqlite3.connect(self.native / (new['conversation_id'] + '.db'))) as c, c:
            c.execute('UPDATE steps SET step_payload=? WHERE idx=1', (b'changed',))
        capture.recover(self.store, attempt)
        self.assertIn('agy_boundary_changed', query.inspect(self.store, attempt)['issues'])

    def test_returned_conversation_mismatch_records_both_and_exit_2(self):
        _, _, original = self.run_case()
        _, code, row = self.run_case('wrong_id', resume=original['conversation_id'])
        self.assertEqual(code, 2)
        self.assertEqual(row['requested_conversation_id'], original['conversation_id'])
        self.assertNotEqual(row['actual_conversation_ids'], [original['conversation_id']])
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        self.assertEqual(row['facts']['process_exit_code'], 0)

    def test_error_without_id_never_searches_other_databases(self):
        with patch.object(agy, 'read_database', side_effect=AssertionError('no ID; no lookup')):
            aid, code, row = self.run_case('no_id')
            capture.recover(self.store, aid)
        self.assertEqual(code, 2)
        self.assertEqual(row['facts']['process_exit_code'], 1)
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertIn('agy_identity_unavailable', row['issues'])

    def test_multiple_results_keep_reports_but_outcome_unknown(self):
        aid, code, row = self.run_case('multiple_results', output=self.root / 'answer')
        self.assertEqual(code, 2)
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
        controls = self.store.load(aid)[2]['control']
        self.assertEqual(sum(e['event'] == 'agy_result' for e in controls), 2)
        self.assertFalse((self.root / 'answer').exists())

    def test_no_result_and_recover_never_infer_completion(self):
        aid, code, row = self.run_case('no_result')
        self.assertEqual(code, 2)
        self.assertIs(row['facts']['target_input_confirmed'], True)
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertEqual(row['auxiliary']['termination_reasons'][0]['name'], 'NO_TOOL_CALL')
        capture.recover(self.store, aid)
        self.assertEqual(query.inspect(self.store, aid)['facts']['turn_status'], 'unknown')
        self.assertEqual(len(self.trace.read_text().splitlines()), 1)

    def test_malformed_result_unknown(self):
        _, code, row = self.run_case('malformed_result')
        self.assertEqual(code, 2)
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')

    def test_result_shapes_unknown_combinations_and_immediate_id_checkpoint(self):
        sid = '00000000-0000-4000-8000-000000000001'
        absent = object()
        for status, reason, response, expected in (
                ('SUCCESS', absent, 'ok', 'completed'), ('ERROR', absent, '', 'failed'),
                ('SUCCESS', 'interrupted', '', 'unknown'), ('ERROR', 'interrupted', '', 'interrupted'),
                ('NEW_STATUS', 'interrupted', '', 'unknown'),
                ('SUCCESS', 'print_timeout', '', 'failed'), ('ERROR', 'print_timeout', '', 'failed'),
                ('NEW_STATUS', 'print_timeout', '', 'unknown'),
                ('SUCCESS', 'future_reason', '', 'unknown'), ('ERROR', 'future_error', '', 'failed'),
                ('SUCCESS', None, '', 'completed'), ('SUCCESS', '', '', 'unknown'),
                ('SUCCESS', 1, '', 'unknown'), ('ERROR', None, '', 'failed'),
                ('NEW_STATUS', 'other', '', 'unknown'), ('SUCCESS', absent, {}, 'unknown')):
            with self.subTest(status=status, reason=reason, response=response):
                event = {'event': 'result', 'result': {'conversation_id': sid, 'status': status,
                         'response': response}}
                if reason is not absent:
                    event['result']['finished_reason'] = reason
                controls = [{'event': name, **value} for name, value in agy.stream_events(event)]
                self.assertEqual(controls[0], {'event': 'agy_identity', 'conversation_id': sid})
                report = agy.reported({'thread_id': None}, controls)
                self.assertEqual(report['turn_status'], expected)
                self.assertEqual(controls[-1]['event'], 'agy_result')
                if reason is absent or reason is None:
                    self.assertIsNone(controls[-1]['finished_reason'])
                else:
                    self.assertIsNotNone(controls[-1]['finished_reason'])
                if expected != 'completed':
                    self.assertIsNone(report['final_message'])
                    self.assertEqual(report['final_output_sha256'], 'unknown')

    def test_recover_unknown_status_returns_exit_2_with_version_check(self):
        aid, _, _ = self.run_case('no_result')
        with patch('subprocess.Popen', wraps=subprocess.Popen) as launch:
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                code = main(['--store', str(self.store.root), 'recover', aid, '--observation-timeout', '0', '--json'])
        self.assertEqual(code, 2)
        self.assertEqual(launch.call_count, 1)
        self.assertEqual(launch.call_args.args[0], [str(FAKE), '--version'])
        self.assertEqual(len(self.trace.read_text().splitlines()), 1)

    def test_interrupted_report_with_user_canceled_is_consistent(self):
        _, code, row = self.run_case('interrupted')
        self.assertEqual(code, 1)
        self.assertEqual(row['facts']['turn_status'], 'interrupted')
        self.assertNotIn('agy_status_conflict', row['issues'])
        self.assertEqual(row['facts']['final_output_sha256'], 'unknown')

    def test_print_timeout_report_with_user_canceled_is_failed(self):
        _, code, row = self.run_case('timeout', extra=['--print-timeout', '1ms'])
        self.assertEqual(code, 3)
        self.assertEqual(row['facts']['turn_status'], 'failed')
        self.assertNotIn('agy_status_conflict', row['issues'])

    def test_success_print_timeout_verified_is_failed_reported(self):
        aid, code, row = self.run_case('success_timeout', extra=['--print-timeout', '1ms'],
                                       output=self.root / 'timeout.md')
        self.assertEqual(code, 2)
        self.assertTrue(row['version_verified'])
        self.assertEqual(row['facts']['process_exit_code'], 0)
        self.assertEqual(row['facts']['turn_status'], 'failed')
        self.assertEqual(row['fact_levels']['turn_status'], 'reported')
        self.assertEqual(row['reported']['turn_status'], 'failed')
        self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
        self.assertEqual(row['auxiliary']['termination_reasons'], [{'value': 2, 'name': 'USER_CANCELED'}])
        self.assertNotIn('agy_status_conflict', row['issues'])
        self.assertFalse((self.root / 'timeout.md').exists())
        result = next(e for e in self.store.load(aid)[2]['control'] if e['event'] == 'agy_result')
        self.assertEqual(result['status'], 'SUCCESS')
        self.assertEqual(result['finished_reason'], 'print_timeout')

    def test_success_print_timeout_unverified_is_failed_reported(self):
        with patch.dict(os.environ, {'FAKE_AGY_VERSION': '9.0.0'}):
            _, code, row = self.run_case('success_timeout', extra=['--print-timeout', '1ms'])
        self.assertEqual(code, 2)
        self.assertFalse(row['version_verified'])
        self.assertEqual(row['facts']['process_exit_code'], 0)
        self.assertEqual(row['facts']['turn_status'], 'failed')
        self.assertEqual(row['fact_levels']['turn_status'], 'reported')
        self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        self.assertIn('agy_version_unverified', row['issues'])
        self.assertNotIn('agy_status_conflict', row['issues'])

    def test_real_sigint_forwarding_to_fake_only(self):
        args = cli_args('--store', str(self.store.root), 'run', str(self.prompt),
                '--engine', 'agy', '--agy', str(FAKE), '--session-root', str(self.native),
                '-C', str(self.root), '--observation-timeout', '0', '--json')
        env = dict(os.environ, FAKE_AGY_MODE='sigint')
        env['PYTHONPATH'] = str(Path(agy.__file__).parents[1])
        with subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env) as proc:
            try:
                deadline = time.monotonic() + 8
                found = False
                while time.monotonic() < deadline and proc.poll() is None:
                    ids = self.store.ids()
                    if ids:
                        controls = self.store.load(ids[0])[2]['control']
                        if any(e['event'] == 'agy_user_input' for e in controls):
                            found = True
                            break
                    time.sleep(.02)
                self.assertTrue(found, 'fake child did not become ready')
                proc.send_signal(signal.SIGINT)
                stdout, stderr = proc.communicate(timeout=8)
                row = json.loads(stdout)
                self.assertEqual(proc.returncode, 1, stderr.decode())
                self.assertEqual(row['facts']['turn_status'], 'interrupted')
                controls = self.store.load(row['attempt_id'])[2]['control']
                self.assertIn('signal_forwarded', [e['event'] for e in controls])
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.communicate()

    def test_success_cancellation_conflict_keeps_both_evidence_sources(self):
        aid, code, row = self.run_case('conflict', output=self.root / 'answer')
        self.assertEqual(code, 2)
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
        self.assertEqual(row['reported']['turn_status'], 'completed')
        self.assertEqual(row['auxiliary']['termination_reasons'][0]['name'], 'USER_CANCELED')
        self.assertIn('agy_status_conflict', row['issues'])
        self.assertFalse((self.root / 'answer').exists())
        self.assertTrue(self.store.load(aid)[2]['target']['segments'][0]['fragments'])

    def test_lf_difference_preserves_reported_output_exactly(self):
        _, _, row = self.run_case('lf_difference')
        self.assertEqual(row['facts']['final_output_sha256'], digest('AGY-ACK\n'))
        self.assertEqual(row['auxiliary']['native_output_sha256'], digest('AGY-ACK'))
        self.assertFalse(row['auxiliary']['matches_reported_output'])

    def test_corrupt_blob_unknown_native_with_valid_report(self):
        _, code, row = self.run_case('corrupt_blob')
        self.assertEqual(code, 0)
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        self.assertEqual(row['facts']['turn_status'], 'completed')

    def test_unknown_fields_are_skipped_without_retention(self):
        aid, _, row = self.run_case('unknown_fields')
        self.assertIs(row['facts']['target_input_confirmed'], True)
        for part in self.store.load(aid)[2]['target']['segments'][0]['fragments']:
            self.assertNotIn(b'UNSELECTED', base64.b64decode(part['wire_base64']))

    def test_duplicate_required_wire_and_sql_disagreement_stay_unknown(self):
        for mode in ('duplicate_field', 'sql_mismatch', 'wrong_wire'):
            with self.subTest(mode=mode):
                _, _, row = self.run_case(mode)
                self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
                self.assertEqual(row['turn_id'], 'unknown')

    def release_fake_writer(self):
        for ready in self.native.glob('ready-*'):
            if not ready.name.endswith('.release'):
                Path(str(ready) + '.release').write_text('release')

    def test_busy_database_reread_within_budget(self):
        original = agy.read_database

        def first_read_then_release(*args, **kw):
            try:
                return original(*args, **kw)
            finally:
                self.release_fake_writer()
        with patch.object(agy, 'read_database', side_effect=first_read_then_release):
            _, code, row = self.run_case('busy', observation_timeout=2)
        self.assertEqual(code, 0)
        self.assertIs(row['facts']['target_input_confirmed'], True)
        self.assertIn('agy_native_busy', [c for a in row['observation_attempts'] for c in a['codes']])
        self.assertGreater(len(row['observation_attempts']), 1)

    def test_delayed_commit_and_recovery_never_resend(self):
        aid, _, row = self.run_case('delayed_commit', observation_timeout=0)
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        self.release_fake_writer()
        capture.recover(self.store, aid, observation_timeout=2)
        self.assertIs(query.inspect(self.store, aid)['facts']['target_input_confirmed'], True)
        self.assertEqual(len(self.trace.read_text().splitlines()), 1)

    def test_late_executor_commit_is_reread_before_conflict_decision(self):
        _, code, row = self.run_case('late_cancel', observation_timeout=2, output=self.root / 'answer')
        self.assertEqual(code, 2)
        self.assertIs(row['facts']['target_input_confirmed'], True)
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertIn('agy_status_conflict', row['issues'])
        self.assertIn('agy_executor_pending', [c for a in row['observation_attempts'] for c in a['codes']])
        self.assertFalse((self.root / 'answer').exists())

    def test_wal_read_transaction_is_ro_and_reads_only_allowed_tables(self):
        original = sqlite3.connect
        statements = []

        def connect(*args, **kw):
            self.assertIn('?mode=ro', args[0])
            self.assertNotIn('immutable', args[0])
            c = original(*args, **kw)
            c.set_trace_callback(statements.append)
            return c
        with patch.object(agy.sqlite3, 'connect', side_effect=connect):
            _, _, row = self.run_case('wal')
        self.assertIs(row['facts']['target_input_confirmed'], True)
        self.assertTrue(any(s == 'BEGIN' for s in statements))
        self.assertFalse(any(word in s.lower() for s in statements for word in ('pragma', 'gen_metadata', 'summary', 'checkpoint')))
        time.sleep(1)  # Allow the synthetic WAL keeper to close before cleanup.

    def assert_unverified(self, row):
        self.assertFalse(row['version_verified'])
        self.assertEqual(row['facts']['turn_status'], 'completed')
        self.assertIs(row['facts']['dispatched'], True)
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        self.assertEqual(row['turn_id'], 'unknown')
        self.assertEqual(row['trajectory_id'], 'unknown')
        self.assertEqual(row['auxiliary']['termination_reasons'], 'unknown')
        self.assertIn('agy_version_unverified', row['issues'])
        self.assertTrue(row['unverified_interpretation'][0]['input_bytes_match'])
        self.assertEqual(row['unverified_interpretation'][0]['assessment'], 'unverified_interpretation')
        self.assertNotIn('USER_CANCELED', canonical(row))

    def test_version_mismatch_dispatches_and_lowers_native_interpretation(self):
        with patch.dict(os.environ, {'FAKE_AGY_VERSION': '9.0.0'}):
            _, code, row = self.run_case('conflict')
        self.assertEqual(code, 0)
        self.assert_unverified(row)
        self.assertEqual(row['unverified_interpretation'][0]['termination_reason_values'], [2])

    def test_hash_mismatch_dispatches_and_keeps_numeric_reason_only(self):
        with patch.object(agy, 'VERIFIED', {**agy.VERIFIED, 'binary_sha256': '0' * 64}):
            _, code, row = self.run_case('conflict')
            self.assertEqual(code, 0)
            self.assert_unverified(row)

    def test_hash_changes_during_execution_lowers_interpretation(self):
        executable = self.root / 'fake_agy_copy.py'
        shutil.copy2(FAKE, executable)
        with patch.dict(os.environ, {'FAKE_AGY_MODE': 'hash_change'}):
            aid = capture.prepare(self.store, self.prompt, engine='agy', agy=executable,
                                  cwd=self.root, session_root=self.native, observation_timeout=0)
            self.assertEqual(capture.execute(self.store, aid), 0)
        row = query.inspect(self.store, aid)
        self.assert_unverified(row)
        hashes = [e['binary_sha256'] for e in row['version_observations']]
        self.assertNotEqual(hashes[0], hashes[1])

    def test_recover_without_after_confirms_input_with_current_fingerprint(self):
        aid, _, _ = self.run_case('no_result')
        state = self.store.load(aid)[2]
        # Reproduce the control evidence available after a wrapper dies at input.
        index = next(i for i, e in enumerate(state['control']) if e['event'] == 'agy_user_input')
        state['control'] = state['control'][:index + 1]
        state['target'] = {'segments': [], 'issues': []}
        self.store.save(aid, state)
        self.assertFalse(query.inspect(self.store, aid)['version_verified'])
        capture.recover(self.store, aid)
        row = query.inspect(self.store, aid)
        self.assertTrue(row['version_verified'])
        self.assertIs(row['facts']['target_input_confirmed'], True)
        self.assertEqual(row['fact_levels']['target_input_confirmed'], 'observed')
        self.assertEqual(row['facts']['turn_status'], 'unknown')
        self.assertEqual(row['facts']['process_exit_code'], 'unknown')
        self.assertNotIn('agy_version_unverified', row['issues'])
        self.assertEqual([e['event'] for e in row['version_observations']],
                         ['agy_version_before', 'agy_version_recover'])
        self.assertEqual(row['version_observations'][-1]['binary_sha256'], digest(FAKE.read_bytes()))
        self.assertEqual(len(self.trace.read_text().splitlines()), 1)

    def test_recover_mismatch_remains_unverified_after_later_matching_recover(self):
        aid, _, _ = self.run_case('no_result')
        state = self.store.load(aid)[2]
        state['control'] = [e for e in state['control'] if e['event'] != 'agy_version_after']
        self.store.save(aid, state)
        with patch.dict(os.environ, {'FAKE_AGY_VERSION': '9.0.0'}):
            capture.recover(self.store, aid)
        capture.recover(self.store, aid)
        row = query.inspect(self.store, aid)
        self.assertFalse(row['version_verified'])
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        recovered = [e for e in row['version_observations'] if e['event'] == 'agy_version_recover']
        self.assertEqual([e['version'] for e in recovered], ['9.0.0', '1.5.9'])
        self.assertEqual(len(self.trace.read_text().splitlines()), 1)

    def test_recover_mismatch_invalidates_verified_after(self):
        aid, _, row = self.run_case('no_result')
        self.assertTrue(row['version_verified'])
        with patch.object(agy, 'fingerprint', return_value={**agy.VERIFIED, 'binary_sha256': '0' * 64}):
            capture.recover(self.store, aid)
        row = query.inspect(self.store, aid)
        self.assertFalse(row['version_verified'])
        self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
        self.assertIn('agy_version_unverified', row['issues'])

    def test_version_verification_requires_before_and_consistent_fingerprints(self):
        def good(name):
            return {'event': 'agy_version_' + name, **agy.VERIFIED,
                    'stat': {field: getattr(FAKE.stat(), field) for field in agy.STAT_FIELDS}}

        before, after, recovered = good('before'), good('after'), good('recover')
        wrong_before = {**before, 'version': '9.0.0'}
        wrong_after = {**after, 'binary_sha256': '0' * 64}
        wrong_recover = {**recovered, 'version': 'unknown'}
        cases = [([], False), ([before], False), ([recovered], False),
                 ([before, after], True), ([before, recovered], True),
                 ([before, recovered, recovered], True), ([before, after, recovered], True),
                 ([before, recovered, wrong_recover], False),
                 ([before, wrong_recover, recovered], False),
                 ([wrong_before, after, recovered], False),
                 ([before, wrong_after, recovered], False),
                 ([before, after, wrong_recover], False)]
        for control, expected in cases:
            with self.subTest(control=control):
                self.assertIs(agy.version_verified(control), expected)

    def test_sensitive_fields_and_full_blobs_never_persist(self):
        aid, _, _ = self.run_case()
        data = b''.join(p.read_bytes() for p in self.store.path(aid).rglob('*') if p.is_file())
        self.assertNotIn(b'UNSELECTED-', data)
        self.assertNotIn(b'NEVER-RETAIN-INIT', data)
        self.assertNotIn(b'fictional.person', data)
        segment = self.store.load(aid)[2]['target']['segments'][0]
        for part in segment['fragments']:
            raw = base64.b64decode(part['wire_base64'])
            self.assertNotIn(b'UNSELECTED-', raw)
            self.assertNotIn(b'fictional.person', raw)
            self.assertLess(len(raw), part['cell_length'])

    def test_sensitive_selected_response_is_omitted_without_reusing_output(self):
        _, _, row = self.run_case('sensitive_response')
        self.assertEqual(row['auxiliary']['native_output_sha256'], 'unknown')
        self.assertEqual(row['facts']['turn_status'], 'completed')
        _, _, row = self.run_case('sensitive_result', output=self.root / 'answer')
        self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
        self.assertFalse((self.root / 'answer').exists())

    def test_stream_step_index_required_in_capture_and_recovery(self):
        for mode in ('no_input_index', 'wrong_index', 'ambiguous_index', 'wrong_body'):
            with self.subTest(mode=mode):
                aid, _, row = self.run_case(mode)
                self.assertEqual(row['facts']['target_input_confirmed'], 'unknown')
                self.assertEqual(row['turn_id'], 'unknown')
                capture.recover(self.store, aid)
                self.assertEqual(query.inspect(self.store, aid)['facts']['target_input_confirmed'], 'unknown')
        for mode in ('repeated_index', 'ambiguous_native'):
            with self.subTest(mode=mode):
                _, _, row = self.run_case(mode)
                self.assertIs(row['facts']['target_input_confirmed'], True)

    def test_init_step_result_id_conflicts_invalidate_report(self):
        _, code, row = self.run_case('different_stream_id')
        self.assertEqual(code, 2)
        self.assertEqual(len(row['actual_conversation_ids']), 2)
        self.assertEqual(row['facts']['turn_status'], 'unknown')

    def test_offline_verify_export_and_projection_consistency(self):
        aid, _, row = self.run_case()
        with patch.object(agy, 'read_database', side_effect=AssertionError('offline query read native')):
            self.assertEqual(query.inspect(self.store, aid), row)
            query.records(self.store, 'agy')
            query.duplicates(self.store, aid)
            query.which(self.store, str(self.prompt))
            export.jsonl(self.store, [aid], self.root / 'evidence.jsonl')
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(main(['--store', str(self.store.root), 'verify', aid, '--json']), 0)
        state = self.store.load(aid)[2]
        state['target']['segments'][0]['steps'][0]['execution_id'] = '00000000-0000-4000-8000-000000000001'
        self.store.save(aid, state)
        with self.assertRaises(ValueError):
            query.inspect(self.store, aid)

    def test_fragment_hash_and_provenance_validation(self):
        for mutation in ('sha256', 'cell_start', 'field_path'):
            with self.subTest(mutation=mutation):
                aid, _, _ = self.run_case()
                state = self.store.load(aid)[2]
                part = state['target']['segments'][0]['fragments'][0]
                part[mutation] = '0' * 64 if mutation == 'sha256' else -1 if mutation == 'cell_start' else [999]
                self.store.save(aid, state)
                with self.assertRaises(ValueError):
                    query.inspect(self.store, aid)

    def test_passthrough_allowlist_and_denials_before_launch(self):
        allowed = ['--mode', 'accept-edits', '--settings={"synthetic":true}', '--disable-slash-commands', '--print-timeout', '1ms']
        aid = self.prepare(extra=allowed)
        self.assertIn('--disable-slash-commands', self.store.load(aid)[1]['argv'])
        denied = [['-p', 'x'], ['-px'], ['--print'], ['--output-format', 'json'], ['--input-format=stream-json'],
                  ['--conversation', 'x'], ['--conversation=x'], ['--resume=x'], ['-c'], ['--model=x'],
                  ['--effort', 'low'], ['--remote-agent'], ['prompt'], ['--'], ['--mode'],
                  ['--disable-slash-commands=true'], ['--settings', '--output-format=json']]
        with patch('subprocess.Popen', side_effect=AssertionError('forbidden launch')):
            for extra in denied:
                with self.subTest(extra=extra), self.assertRaises(ValueError):
                    self.prepare(extra=extra)
        self.assertFalse(self.trace.exists())

    def test_e4_passthrough_rejects_dash_values_before_launch(self):
        with patch('subprocess.Popen', side_effect=AssertionError('forbidden launch')):
            for option in ('--mode', '--settings', '--print-timeout'):
                for value in ('-p', '--output-format=json', '-', '--'):
                    for extra in ([option + '=' + value], [option, value]):
                        with self.subTest(extra=extra), self.assertRaises(ValueError):
                            self.prepare(extra=extra)
        self.assertEqual(self.store.ids(), [])
        self.assertFalse(self.trace.exists())

    def test_e4_equals_passthrough_preserves_one_token(self):
        extras = ['--mode=accept-edits', '--settings={"synthetic":"a=b c"}',
                  '--print-timeout=1ms', '--disable-slash-commands']
        attempt = self.prepare(extra=extras)
        argv = self.store.load(attempt)[1]['argv']
        self.assertEqual(argv[-len(extras):], extras)
        spaced = ['--mode', 'accept-edits', '--settings', '{"synthetic":"a=b c"}',
                  '--print-timeout', '1ms']
        self.assertEqual(agy.extra_arguments(spaced), spaced)

    def test_cli_forbidden_passthrough_is_unsent_and_help_lists_engine(self):
        for extra in (['-p', 'x'], ['--output-format=json'], ['--conversation=x']):
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                code = main(['--store', str(self.store.root), 'run', str(self.prompt), '--engine', 'agy', '--agy', str(FAKE),
                             '--session-root', str(self.native), '--json', '--', *extra])
            self.assertEqual(code, 2)
        self.assertFalse(self.trace.exists())
        self.assertEqual(self.store.ids(), [])
        stream = io.StringIO()
        with contextlib.redirect_stdout(stream), self.assertRaises(SystemExit):
            parser().parse_args(['run', '--help'])
        self.assertIn('--agy', stream.getvalue())
        self.assertIn('codex,claude,agy', stream.getvalue())

    def test_invalid_utf8_daemon_thread_and_cross_engine_retry_refused(self):
        for kw in ({'via': 'daemon'}, {'thread': 'x'}):
            with self.assertRaises(ValueError):
                self.prepare(**kw)
        aid, _, _ = self.run_case()
        with self.assertRaisesRegex(ValueError, 'same engine'):
            capture.prepare(self.store, self.prompt, engine='claude', claude=FAKE,
                            cwd=self.root, session_root=self.root / 'claude', retry_of=aid)
        self.prompt.write_bytes(b'\xff')
        with patch('subprocess.Popen', side_effect=AssertionError('invalid UTF-8 launch')):
            with self.assertRaisesRegex(ValueError, 'UTF-8'):
                self.prepare()

    def test_long_unicode_stream_input_keeps_exact_bytes(self):
        self.prompt.write_text(' \n' + ('\ud55c\uae00 e\u0301 café 🧪\t' * 2000) + '\n ')
        _, code, row = self.run_case()
        self.assertEqual(code, 0)
        self.assertIs(row['facts']['target_input_confirmed'], True)


class WireFixtureTest(unittest.TestCase):
    def test_production_verified_constants(self):
        self.assertEqual(PRODUCTION_VERIFIED, {'version': '1.5.9', 'binary_sha256':
                         '39816d49d5ab92229952987225c23ea3fed7580699dcb4a444e0388e0ac1ce43'})

    def test_fixed_wire_fixture_independent_of_encoder(self):
        fixture = json.loads(FIXTURE.read_text())
        execution = '00000000-0000-4000-8000-000000000001'
        metadata = bytes.fromhex(fixture['metadata_hex'])
        user, parts = agy.step_projection((0, 14, 3, metadata, bytes.fromhex(fixture['user_hex'])))
        planner, _ = agy.step_projection((1, 15, 3, metadata, bytes.fromhex(fixture['planner_hex'])))
        executor, _ = agy.executor_projection((0, bytes.fromhex(fixture['executor_hex'])))
        self.assertEqual(user['execution_id'], execution)
        self.assertEqual(user['input_sha256'], digest('A'))
        self.assertEqual(planner['response_sha256'], digest('B'))
        self.assertEqual(executor['termination_reason'], 4)
        for part in parts:
            agy.validate_fragment(part)

    def test_wire_reader_rejects_truncation_invalid_tags_and_duplicates(self):
        for raw in (b'\x80', b'\x00', b'\x0b', b'\x0a\x03x', b'\x09x', b'\x0dxx', b'\x08' + b'\xff' * 10):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                fields(raw)
        with self.assertRaises(ValueError):
            one(fields(b'\x08\x01\x08\x02'), 1, 0)
        with self.assertRaises(ValueError):
            one(fields(b'\x0a\x00'), 1, 0)
        self.assertEqual(one(nested(one(fields(b'\x0a\x02\x08\x01'), 1, 2)), 1, 0).value, 1)


if __name__ == '__main__':
    unittest.main()
