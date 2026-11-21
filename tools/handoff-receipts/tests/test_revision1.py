"""Regression coverage for atomic publication, dispatch refusal and Python 3.10."""
import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from python_fakes import enable_python_fakes

from handoff_receipts import capture, query
from handoff_receipts.cli import main, parser
from handoff_receipts.common import digest, parse_timestamp
from handoff_receipts.rpc import Rpc
from handoff_receipts.store import Store
from fake_daemon import FakeDaemon

SRC = Path(__file__).resolve().parents[1] / 'src'
FAKE = Path(__file__).with_name('fake_exec.py').resolve()
NOT_LOADED = 'thread not loaded in the daemon; open it in the Codex app or use --via exec --resume'


class RevisionOneTest(unittest.TestCase):
    def setUp(self):
        enable_python_fakes(self)
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.store = Store(self.root / 'store')
        self.instruction = self.root / 'input.txt'
        self.instruction.write_text('Synthetic revision test.\n')
        self.sessions = self.root / 'sessions'
        self.env = patch.dict(os.environ, {'FAKE_SESSIONS': str(self.sessions), 'FAKE_MODE': 'completed'})
        self.env.start()

    def tearDown(self):
        self.env.stop()
        self.temp.cleanup()

    def prepare(self, **kw):
        kw.setdefault('observation_timeout', 0)
        return capture.prepare(self.store, self.instruction, codex=FAKE, cwd=self.root,
                               session_root=self.sessions, **kw)

    def run_exec(self, **kw):
        aid = self.prepare(**kw)
        self.assertEqual(capture.execute(self.store, aid), 0)
        return aid

    def cli(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = main(['--store', str(self.store.root), *args])
        return code, out.getvalue(), err.getvalue()

    def test_publish_only_after_complete_seal_and_checkpoint(self):
        rename = os.rename
        def observe(staging, final):
            self.assertEqual(self.store.ids(), [])
            self.assertFalse(final.exists())
            self.assertTrue(staging.name.startswith('.pending-'))
            for name in ('body.txt', 'source.json', 'request.json', 'manifest.json', 'INTERPRETATION.md', 'CURRENT'):
                self.assertTrue((staging / name).is_file())
            head = (staging / 'CURRENT').read_text().strip()
            self.assertEqual(digest((staging / 'states' / (head + '.json')).read_bytes()), head)
            rename(staging, final)
        with patch('handoff_receipts.store.os.rename', side_effect=observe) as publish:
            aid = self.prepare()
        self.assertEqual(publish.call_count, 1)
        self.assertEqual(self.store.ids(), [aid])
        self.assertEqual(list(self.store.root.glob('.pending-*')), [])
        self.assertFalse(query.inspect(self.store, aid)['facts']['attempted'])

    def test_crash_before_publication_is_invisible(self):
        script = '''
import os, sys
from unittest.mock import patch
from handoff_receipts.store import Store
with patch('handoff_receipts.store.os.rename', side_effect=lambda *a: os._exit(77)):
    Store(sys.argv[1]).create(sys.argv[2], {'via': 'exec'})
'''
        proc = subprocess.run([sys.executable, '-c', script, str(self.store.root), str(self.instruction)],
                              env={**os.environ, 'PYTHONPATH': str(SRC)}, capture_output=True)
        self.assertEqual(proc.returncode, 77)
        self.assertEqual(self.store.ids(), [])
        self.assertEqual(len(list(self.store.root.glob('.pending-*'))), 1)
        code, out, err = self.cli('list', '--json')
        self.assertEqual((code, json.loads(out), err), (0, [], ''))

    def test_creation_error_leaves_no_published_bundle(self):
        with patch('handoff_receipts.store.write_json', side_effect=OSError('synthetic disk failure')):
            with self.assertRaises(OSError):
                self.prepare()
        self.assertEqual(self.store.ids(), [])
        self.assertEqual(list(self.store.root.glob('.pending-*')), [])

    def test_broken_bundles_warn_without_breaking_queries_or_forget(self):
        good = self.run_exec()
        bad = self.run_exec()
        (self.store.path(bad) / 'body.txt').write_text('tampered')
        partial = 'f' * 32
        self.store.path(partial).mkdir()
        (self.store.path(partial) / 'source.json').write_text('{')
        commands = [('list',), ('inspect', good), ('inspect', bad), ('dup', good), ('dup', bad),
                    ('which', str(self.instruction))]
        for command in commands:
            with self.subTest(command=command):
                code, out, err = self.cli(*command, '--json')
                self.assertEqual(code, 0)
                result = json.loads(out)
                warnings = [json.loads(line) for line in err.splitlines()]
                self.assertEqual({w['attempt_id'] for w in warnings}, {bad, partial})
                self.assertTrue(all(w['warning'] == 'broken_bundle_skipped' for w in warnings))
                if command == ('list',):
                    self.assertEqual([r['attempt_id'] for r in result], [good])
                if command == ('dup', good):
                    self.assertEqual(result['effective_attempt'], 'unknown')
                    self.assertEqual(result['duplicate_execution'], 'unknown')
        self.assertEqual(self.cli('verify', '--json')[0], 2)
        for aid in (bad, partial):
            self.assertEqual(self.cli('forget', aid, '--json')[0], 0)
            self.assertFalse(self.store.path(aid).exists())
        self.assertEqual(query.inspect(self.store, good)['facts']['turn_status'], 'completed')

    def test_forget_still_refuses_active_capture(self):
        aid = self.prepare()
        with self.store.lock(aid):
            with self.assertRaisesRegex(ValueError, 'still being'):
                self.store.forget(aid)
        self.assertTrue(self.store.path(aid).is_dir())

    def test_utc_z_parser_and_duplicate_order(self):
        self.assertEqual(parse_timestamp('2026-11-01T01:02:03.456Z'),
                         parse_timestamp('2026-11-01T01:02:03.456+00:00'))
        with patch.dict(os.environ, {'FAKE_UTC_Z': '1'}):
            first = self.run_exec()
            second = self.run_exec(retry_of=first)
        self.assertTrue(query.inspect(self.store, first)['facts']['completed_at'].endswith('Z'))
        result = query.duplicates(self.store, second)
        self.assertEqual(result['effective_attempt'], first)
        self.assertTrue(result['duplicate_execution'])

    def daemon_thread(self, fake):
        client = Rpc(fake.socket)
        thread = client.call('thread/start', {'cwd': str(self.root)})['thread']['id']
        client.close()
        return thread

    def daemon_prepare(self, fake, thread):
        return capture.prepare(self.store, self.instruction, via='daemon', thread=thread,
                               socket=fake.socket, session_root=self.sessions, cwd=self.root)

    def test_unmaterialized_thread_with_missing_file_or_path(self):
        for hidden in (False, True):
            with self.subTest(hidden_path=hidden), FakeDaemon(self.sessions, self.root / 'socket') as fake:
                fake.defer_rollout, fake.hide_rollout_path = True, hidden
                thread = self.daemon_thread(fake)
                self.assertFalse(Path(fake.threads[thread]['path']).exists())
                aid = self.daemon_prepare(fake, thread)
                self.assertEqual(capture.execute(self.store, aid), 0)
                row = query.inspect(self.store, aid)
                self.assertTrue(row['facts']['attempted'])
                self.assertTrue(row['facts']['dispatched'])
                self.assertTrue(row['facts']['target_input_confirmed'])
                self.assertEqual(row['facts']['turn_status'], 'completed')
                state = self.store.load(aid)[2]
                self.assertEqual(state['target']['preflight'], {'bytes': 0, 'sha256': digest(b'')})
                self.assertEqual(sum(c['method'] == 'turn/start' for c in fake.calls), 1)
                self.assertTrue(any(e['event'] == 'rpc_error' and e['method'] == 'thread/turns/list'
                                    for e in state['control']))

    def test_not_loaded_refusal_message_and_no_dispatch(self):
        with FakeDaemon(self.sessions, self.root / 'socket') as fake:
            thread = self.daemon_thread(fake)
            fake.threads[thread]['status'] = {'type': 'notLoaded'}
            aid = self.daemon_prepare(fake, thread)
            with patch('handoff_receipts.capture.read_native', side_effect=AssertionError('no native reread')):
                self.assertEqual(capture.execute(self.store, aid), 2)
                capture.recover(self.store, aid)
            row = query.inspect(self.store, aid)
            self.assertEqual(row['rejection_reason'], NOT_LOADED)
            self.assertFalse(row['facts']['attempted'])
            self.assertFalse(row['facts']['dispatched'])
            self.assertFalse(any(c['method'] in ('turn/start', 'thread/resume') for c in fake.calls))
            for command in (('inspect', aid), ('list',)):
                code, out, err = self.cli(*command)
                self.assertEqual(code, 0)
                self.assertIn('not sent', out)
                self.assertIn(NOT_LOADED, out)
                self.assertIn('dispatched: False', out)

    def test_missing_rollout_of_existing_thread_is_not_new(self):
        with FakeDaemon(self.sessions, self.root / 'socket') as fake:
            thread = self.daemon_thread(fake)
            fake.threads[thread]['turns'].append({'id': 'prior', 'status': 'completed'})
            Path(fake.threads[thread]['path']).unlink()
            aid = self.daemon_prepare(fake, thread)
            self.assertEqual(capture.execute(self.store, aid), 2)
            row = query.inspect(self.store, aid)
            self.assertFalse(row['facts']['dispatched'])
            self.assertIn('existing thread', row['rejection_reason'])
            self.assertFalse(any(c['method'] == 'turn/start' for c in fake.calls))

    def test_old_intent_event_is_not_dispatch_evidence(self):
        aid = self.prepare(via='daemon', thread='synthetic-thread')
        state = self.store.load(aid)[2]
        state['control'][0]['event'] = 'attempted'  # initial v0.1 event vocabulary
        state['control'].append({'event': 'daemon_error', 'category': 'ValueError'})
        self.store.save(aid, state)
        row = query.inspect(self.store, aid)
        self.assertFalse(row['facts']['attempted'])
        self.assertFalse(row['facts']['dispatched'])
        self.assertIn('legacy bundle', row['rejection_reason'])

    def test_socket_default_and_failed_process_not_sent(self):
        expected = Path.home() / '.codex/app-server-control/app-server-control.sock'
        self.assertEqual(parser().parse_args(['run', str(self.instruction)]).socket, expected)
        aid = self.prepare(via='daemon', thread='synthetic-thread')
        self.assertEqual(self.store.load(aid)[1]['socket'], str(expected))
        aid = self.prepare()
        with patch('handoff_receipts.capture.subprocess.Popen', side_effect=FileNotFoundError):
            self.assertEqual(capture.execute(self.store, aid), 127)
        row = query.inspect(self.store, aid)
        self.assertFalse(row['facts']['attempted'])
        self.assertFalse(row['facts']['dispatched'])
        self.assertIsNotNone(row['rejection_reason'])


if __name__ == '__main__':
    unittest.main()
