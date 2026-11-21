"""Synthetic stdout failures; no native executables or session data are used."""
import json
import signal
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from handoff_receipts import agy, capture, query
from handoff_receipts.store import Store


class CaptureStreamErrorTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.store = Store(self.root / 'store')
        self.prompt = self.root / 'input.txt'
        self.prompt.write_text('Synthetic stream error instruction.\n')

    def prepare(self, engine):
        output = self.root / (engine + '.md')
        attempt = capture.prepare(self.store, self.prompt, engine=engine,
                                  agy=sys.executable, claude=sys.executable,
                                  cwd=self.root, session_root=self.root / 'native',
                                  observation_timeout=0, output=output)
        request = self.store.load(attempt)[1]
        sid = request['thread_id'] or '00000000-0000-4000-8000-000000000001'
        if engine == 'agy':
            event = {'event': 'result', 'result': {'conversation_id': sid,
                     'status': 'SUCCESS', 'response': 'ACK'}}
        else:
            event = {'type': 'result', 'session_id': sid, 'is_error': False,
                     'terminal_reason': 'completed', 'result': 'ACK'}
        return attempt, (json.dumps(event) + '\n').encode(), output

    def execute(self, attempt, chunks, *, exit_code=0, running=False):
        process = Mock(spec=subprocess.Popen)
        process.pid = 12345
        process.stdout = Mock()
        process.stdout.fileno.return_value = 42
        process.poll.return_value = None if running else exit_code
        process.wait.return_value = exit_code
        selector = Mock()
        selector.get_map.side_effect = [True] * len(chunks) + [False]
        selector.select.return_value = [(SimpleNamespace(fileobj=process.stdout), 1)]
        with (patch.object(capture.subprocess, 'Popen', return_value=process),
              patch.object(capture.selectors, 'DefaultSelector', return_value=selector),
              patch.object(capture.os, 'read', side_effect=chunks),
              patch.object(capture.os, 'killpg') as killpg,
              patch.object(agy, 'fingerprint', return_value=dict(agy.VERIFIED))):
            code = capture.execute(self.store, attempt)
        return code, process, killpg, selector

    def test_e1_read_error_invalidates_prior_success(self):
        for engine in ('agy', 'claude'):
            with self.subTest(engine=engine):
                attempt, success, output = self.prepare(engine)
                code, _, _, _ = self.execute(attempt, [success, OSError('synthetic read failure')])
                row = query.inspect(self.store, attempt)
                self.assertEqual(row['facts']['turn_status'], 'unknown')
                self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
                self.assertFalse(output.exists())
                self.assertEqual(code, 2 if engine == 'agy' else 127)
                control = self.store.load(attempt)[2]['control']
                names = [event['event'] for event in control]
                self.assertIn('process_error', names)
                self.assertIn(engine + '_stream_incomplete', names)
                self.assertIn(engine + '_result', names)

    def test_e1_read_error_stops_and_reaps_live_child(self):
        for engine in ('agy', 'claude'):
            with self.subTest(engine=engine):
                attempt, success, _ = self.prepare(engine)
                _, process, killpg, selector = self.execute(
                    attempt, [success, OSError('synthetic read failure')],
                    running=True, exit_code=-signal.SIGTERM)
                killpg.assert_called_once()
                self.assertEqual(killpg.call_args.args[0], process.pid)
                process.wait.assert_called_once()
                selector.close.assert_called_once()
                process.stdout.close.assert_called_once()

    def test_e2_deep_json_is_unrecognized_and_capture_continues(self):
        depth = max(100_000, sys.getrecursionlimit() * 2)
        deep = b'[' * depth + b'0' + b']' * depth + b'\n'
        # Depending on the Python build, this line either raises RecursionError or
        # decodes to a nested list; both must be recorded as unrecognized.
        for engine in ('agy', 'claude'):
            for before_success in (True, False):
                with self.subTest(engine=engine, before_success=before_success):
                    attempt, success, output = self.prepare(engine)
                    chunks = [deep, success] if before_success else [success, deep]
                    code, process, killpg, selector = self.execute(attempt, [*chunks, b''])
                    self.assertEqual(code, 2 if engine == 'agy' else 0)
                    control = self.store.load(attempt)[2]['control']
                    names = [event['event'] for event in control]
                    self.assertIn('stream_unrecognized', names)
                    self.assertIn(engine + '_stream_incomplete', names)
                    self.assertIn(engine + '_result', names)
                    self.assertIn('process_exited', names)
                    row = query.inspect(self.store, attempt)
                    self.assertEqual(row['facts']['turn_status'], 'unknown')
                    self.assertEqual(row['facts']['final_output_sha256'], 'unknown')
                    self.assertFalse(output.exists())
                    process.wait.assert_called_once()
                    killpg.assert_not_called()
                    selector.close.assert_called_once()
                    process.stdout.close.assert_called_once()

    def test_e3_codex_non_object_json_is_unrecognized_and_capture_continues(self):
        depth = max(100_000, sys.getrecursionlimit() * 2)
        lines = (b'[0]\n', b'5\n', b'"text"\n', b'null\n',
                 b'[' * depth + b'0' + b']' * depth + b'\n')
        for line in lines:
            with self.subTest(line=line[:8]):
                output = self.root / 'codex.md'
                attempt = capture.prepare(self.store, self.prompt, codex=sys.executable,
                                          cwd=self.root, session_root=self.root / 'native',
                                          observation_timeout=0, output=output)
                code, process, killpg, selector = self.execute(attempt, [line, b''])
                self.assertEqual(code, 0)
                names = [event['event'] for event in self.store.load(attempt)[2]['control']]
                self.assertIn('stream_unrecognized', names)
                self.assertIn('process_exited', names)
                self.assertFalse(output.exists())
                process.wait.assert_called_once()
                killpg.assert_not_called()
                selector.close.assert_called_once()
                process.stdout.close.assert_called_once()


if __name__ == '__main__':
    unittest.main()
