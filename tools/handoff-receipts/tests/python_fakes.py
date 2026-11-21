"""Run synthetic producers through Python without executable source files."""
from pathlib import Path
import subprocess
import sys
from unittest.mock import patch


def python_fakes(extra=()):
    scripts = {Path(__file__).with_name(name).resolve() for name in (
        'fake_exec.py', 'fake_claude.py', 'fake_agy.py',
    )}
    scripts.update(Path(path).resolve() for path in extra)
    popen = subprocess.Popen

    def launch(args, *positional, **kwargs):
        if isinstance(args, (list, tuple)) and args and Path(args[0]).resolve() in scripts:
            args = [sys.executable, *args]
        return popen(args, *positional, **kwargs)

    return patch.object(subprocess, 'Popen', new=launch)


def enable_python_fakes(test, extra=()):
    launcher = python_fakes(extra)
    launcher.start()
    test.addCleanup(launcher.stop)


def cli_args(*args):
    # Separate CLI processes need the same test-only subprocess adapter.
    return [sys.executable, str(Path(__file__).resolve()), *args]


if __name__ == '__main__':
    from handoff_receipts.cli import main

    with python_fakes():
        raise SystemExit(main())
