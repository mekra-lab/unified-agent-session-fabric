"""Caller-managed raw stderr, using synthetic executables for all engines."""
import contextlib
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from python_fakes import enable_python_fakes

from handoff_receipts import agy, capture
from handoff_receipts.cli import main, parser
from handoff_receipts.common import digest
from handoff_receipts.store import Store

FAKES = {
    'codex': Path(__file__).with_name('fake_exec.py').resolve(),
    'claude': Path(__file__).with_name('fake_claude.py').resolve(),
    'agy': Path(__file__).with_name('fake_agy.py').resolve(),
}
DIAGNOSTIC = (b'jetski: synthetic permission denial\r\n'
              b'account=stderr-only@example.invalid\x00\xff\xfe\nunterminated')


class ChildStderrTest(unittest.TestCase):
    def setUp(self):
        enable_python_fakes(self)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.store = Store(self.root / 'store')
        self.prompt = self.root / 'input.txt'
        self.prompt.write_text('Synthetic stderr capture instruction.\n')
        self.stderr_path = self.root / 'child-stderr.log'
        self.native = {engine: self.root / engine for engine in FAKES}
        self.env = patch.dict(os.environ, {
            'FAKE_CHILD_STDERR_HEX': DIAGNOSTIC.hex(),
            'FAKE_SESSIONS': str(self.native['codex']), 'FAKE_MODE': 'completed',
            'FAKE_CLAUDE_PROJECTS': str(self.native['claude']), 'FAKE_CLAUDE_MODE': 'completed',
            'FAKE_AGY_ROOT': str(self.native['agy']), 'FAKE_AGY_MODE': 'normal',
            'FAKE_AGY_VERSION': '1.5.9',
        })
        self.env.start()
        self.addCleanup(self.env.stop)
        verified = patch.object(agy, 'VERIFIED', {
            'version': '1.5.9', 'binary_sha256': digest(FAKES['agy'].read_bytes()),
        })
        verified.start()
        self.addCleanup(verified.stop)

    def cli(self, engine='codex', extra=()):
        out, err = io.StringIO(), io.StringIO()
        args = ['--store', str(self.store.root), 'run', str(self.prompt),
                '--engine', engine, '--' + engine, str(FAKES[engine]),
                '--cwd', str(self.root), '--session-root', str(self.native[engine]),
                '--observation-timeout', '0', '--json', *map(str, extra)]
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = main(args)
        return code, json.loads(out.getvalue()), err.getvalue()

    def assert_capture(self, engine):
        code, receipt, wrapper_stderr = self.cli(engine, ['--child-stderr', self.stderr_path])
        self.assertEqual(code, 0)
        self.assertEqual(receipt['facts']['turn_status'], 'completed')
        self.assertEqual(self.stderr_path.read_bytes(), DIAGNOSTIC)
        self.assertEqual(self.stderr_path.stat().st_mode & 0o777, 0o600)
        self.assertIn('attempt_id=', wrapper_stderr)
        self.assertNotIn('stderr-only@example.invalid', wrapper_stderr)
        self.assertNotIn(str(self.stderr_path), json.dumps(receipt))
        for path in self.store.root.rglob('*'):
            if path.is_file():
                raw = path.read_bytes()
                self.assertNotIn(b'stderr-only@example.invalid', raw)
                self.assertNotIn(str(self.stderr_path).encode(), raw)
                self.assertNotIn(b'child_stderr', raw)

    def test_codex_raw_stderr(self):
        self.assert_capture('codex')

    def test_claude_raw_stderr(self):
        self.assert_capture('claude')

    def test_agy_raw_stderr_excludes_version_probes(self):
        # The fake writes the same stderr for --version, before and after dispatch.
        self.assert_capture('agy')

    def test_default_discards_stderr_for_all_engines(self):
        for engine in FAKES:
            with self.subTest(engine=engine):
                with patch('handoff_receipts.capture.subprocess.Popen', wraps=subprocess.Popen) as launch:
                    code, _, wrapper_stderr = self.cli(engine)
                self.assertEqual(code, 0)
                self.assertTrue(launch.call_args_list)
                self.assertTrue(all(call.kwargs['stderr'] == subprocess.DEVNULL
                                    for call in launch.call_args_list))
                self.assertNotIn('stderr-only@example.invalid', wrapper_stderr)
                self.assertFalse(self.stderr_path.exists())

    def test_overwrites_existing_file_and_resets_permissions(self):
        self.stderr_path.write_bytes(b'old diagnostics' * 100)
        self.stderr_path.chmod(0o666)
        self.assert_capture('codex')

    def test_large_stderr_is_preserved_without_pipe_backpressure(self):
        diagnostic = DIAGNOSTIC * 700
        with patch.dict(os.environ, {'FAKE_CHILD_STDERR_HEX': diagnostic.hex()}):
            code, _, _ = self.cli(extra=['--child-stderr', self.stderr_path])
        self.assertEqual(code, 0)
        self.assertEqual(self.stderr_path.read_bytes(), diagnostic)

    def test_failed_child_still_writes_stderr(self):
        with patch.dict(os.environ, {'FAKE_MODE': 'failed'}):
            code, receipt, _ = self.cli(extra=['--child-stderr', self.stderr_path])
        self.assertEqual(code, 1)
        self.assertEqual(receipt['facts']['process_exit_code'], 1)
        self.assertEqual(self.stderr_path.read_bytes(), DIAGNOSTIC)

    def test_agy_without_result_still_writes_stderr(self):
        with patch.dict(os.environ, {'FAKE_AGY_MODE': 'no_result'}):
            code, receipt, _ = self.cli('agy', ['--child-stderr', self.stderr_path])
        self.assertEqual(code, 2)
        self.assertEqual(receipt['facts']['turn_status'], 'unknown')
        self.assertEqual(self.stderr_path.read_bytes(), DIAGNOSTIC)

    def assert_refused(self, path, extra=()):
        with patch('handoff_receipts.capture.subprocess.Popen') as launch:
            code, result, wrapper_stderr = self.cli(extra=['--child-stderr', path, *extra])
        self.assertEqual(code, 2)
        self.assertIn('error', result)
        self.assertNotIn('attempt_id=', wrapper_stderr)
        self.assertEqual(self.store.ids(), [])
        launch.assert_not_called()
        return result

    def test_missing_parent_refused_before_attempt(self):
        path = self.root / 'missing' / 'child.log'
        self.assert_refused(path)
        self.assertFalse(path.parent.exists())

    def test_unwritable_parent_refused_before_attempt(self):
        folder = self.root / 'read-only'
        folder.mkdir()
        folder.chmod(0o500)
        try:
            self.assert_refused(folder / 'child.log')
        finally:
            folder.chmod(0o700)

    def test_unwritable_parent_with_writable_file_refused_before_attempt(self):
        folder = self.root / 'read-only'
        folder.mkdir()
        path = folder / 'child.log'
        path.write_bytes(b'keep existing diagnostics')
        folder.chmod(0o500)
        try:
            self.assert_refused(path)
            self.assertEqual(path.read_bytes(), b'keep existing diagnostics')
        finally:
            folder.chmod(0o700)

    def test_unwritable_file_refused_before_attempt(self):
        self.stderr_path.write_bytes(b'keep existing diagnostics')
        self.stderr_path.chmod(0o400)
        try:
            self.assert_refused(self.stderr_path)
            self.assertEqual(self.stderr_path.read_bytes(), b'keep existing diagnostics')
        finally:
            self.stderr_path.chmod(0o600)

    def test_directory_destination_refused_before_attempt(self):
        self.assert_refused(self.root)

    def test_store_destination_refused_before_attempt(self):
        result = self.assert_refused(self.store.root / 'child.log')
        self.assertEqual(result['error'], 'child stderr must be outside the receipt store')
        self.assertFalse(self.store.root.exists())

    def test_symlink_to_store_refused_without_overwriting(self):
        self.store.root.mkdir()
        target = self.store.root / 'keep.txt'
        target.write_bytes(b'keep receipt bytes')
        self.stderr_path.symlink_to(target)
        self.assert_refused(self.stderr_path)
        self.assertEqual(target.read_bytes(), b'keep receipt bytes')

    def test_daemon_option_refused_before_attempt(self):
        result = self.assert_refused(self.stderr_path, ['--via', 'daemon', '--thread', 'synthetic-thread'])
        self.assertEqual(result['error'], '--child-stderr requires --via exec')
        self.assertFalse(self.stderr_path.exists())

    def test_other_commands_do_not_accept_option(self):
        for args in (['recover', '0' * 32], ['inspect', '0' * 32], ['list'],
                     ['verify'], ['dup', '0' * 32], ['which', 'synthetic'],
                     ['forget', '0' * 32], ['export', '--format', 'jsonl', '-o', 'export.jsonl']):
            with self.subTest(command=args[0]), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    parser().parse_args([*args, '--child-stderr', str(self.stderr_path)])
                self.assertEqual(error.exception.code, 2)

    def test_diagnostic_file_closes_when_prepare_fails(self):
        with self.assertRaisesRegex(ValueError, 'synthetic refusal'):
            with capture.child_stderr_file(self.store, self.stderr_path) as stderr:
                self.assertFalse(stderr.closed)
                raise ValueError('synthetic refusal')
        self.assertTrue(stderr.closed)
        with patch('handoff_receipts.cli.capture.prepare', side_effect=ValueError('synthetic refusal')):
            self.assert_refused(self.stderr_path)


if __name__ == '__main__':
    unittest.main()
