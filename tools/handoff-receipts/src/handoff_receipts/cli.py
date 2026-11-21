"""Command-line interface. Errors are classified without echoing private data."""
import argparse
import json
import sys
from pathlib import Path
from . import __version__
from .common import canonical, no_sensitive
from .store import Store
from . import capture, export, query


def parser():
    p = argparse.ArgumentParser(prog='handoff-receipts', description='Capture and inspect local Codex, Claude and experimental Antigravity handoff evidence.',
                                allow_abbrev=False)
    p.add_argument('--version', action='version', version=__version__)
    p.add_argument('--store', type=Path, help='data store (default: $XDG_DATA_HOME/handoff-receipts)')
    sub = p.add_subparsers(dest='cmd', required=True)
    run = sub.add_parser('run', help='send one instruction file; never automatically retry',
                         allow_abbrev=False)
    run.add_argument('instruction', type=Path)
    run.add_argument('--engine', choices=['codex', 'claude', 'agy'], default='codex',
                     help='target engine (agy is experimental)')
    run.add_argument('--claude', default='claude', help='Claude executable')
    run.add_argument('--agy', default='agy', help='experimental Antigravity executable')
    run.add_argument('--via', choices=['exec', 'daemon'], default='exec')
    run.add_argument('--resume', '-r')
    run.add_argument('--thread')
    run.add_argument('--socket', type=Path, default=capture.default_socket(),
                     help='daemon socket (default: ~/.codex/app-server-control/app-server-control.sock)')
    run.add_argument('--retry-of')
    run.add_argument('-m', '--model')
    run.add_argument('-e', '--effort')
    run.add_argument('-C', '--cwd', type=Path, default=Path.cwd())
    run.add_argument('-o', '--output', type=Path, help='final message file, or output directory with -n')
    run.add_argument('--child-stderr', type=Path, metavar='PATH',
                     help='write raw child stderr to a caller-managed file (exec only)')
    run.add_argument('-n', '--name', help='label; with -o writes OUTPUT/NAME.md')
    run.add_argument('--codex', default='codex', help='Codex executable')
    run.add_argument('--session-root', type=Path, help='native sessions directory (advanced)')
    for name in ('inspect', 'dup', 'which', 'recover', 'forget'):
        sp = sub.add_parser(name)
        sp.add_argument('selector')
        sp.add_argument('--json', action='store_true')
        if name in ('inspect', 'dup', 'which'):
            sp.add_argument('--engine', choices=['codex', 'claude', 'agy'])
        if name == 'recover':
            sp.add_argument('--output', '-o', type=Path, help='override the saved answer destination')
            sp.add_argument('--observation-timeout', type=float, default=None)
    listing = sub.add_parser('list')
    listing.add_argument('--engine', choices=['codex', 'claude', 'agy'])
    listing.add_argument('--json', action='store_true')
    ver = sub.add_parser('verify')
    ver.add_argument('selector', nargs='?')
    ver.add_argument('--json', action='store_true')
    ex = sub.add_parser('export')
    ex.add_argument('selector', nargs='?')
    ex.add_argument('--format', choices=['jsonl', 'polylogue'], required=True)
    ex.add_argument('--output', '-o', type=Path, required=True)
    ex.add_argument('--polylogue-bridge', type=Path)
    ex.add_argument('--json', action='store_true')
    run.add_argument('--observation-timeout', type=float, default=capture.DEFAULT_OBSERVATION_TIMEOUT,
                     help='native reread budget in seconds (0..30, default: 2)')
    run.add_argument('--json', action='store_true')
    return p


def emit(value, machine):
    encoded = canonical(value)
    no_sensitive(encoded)
    if machine:
        print(encoded)
        return
    if isinstance(value, list) and all('facts' in r for r in value):
        if not value:
            print('No recorded attempts.')
        for row in value:
            facts = row['facts']
            delivery = 'not sent' if facts['dispatched'] is False else f"{facts['turn_status']} ({row['fact_levels']['turn_status']})"
            print(f"{row['attempt_id']}  {delivery}  {row['name'] or ''}")
            if row.get('rejection_reason'):
                print('  reason: ' + row['rejection_reason'])
            if row.get('engine') == 'claude':
                print('  engine: claude  turn key kind: user_message')
            if row.get('engine') == 'agy':
                print('  engine: agy (experimental)  turn key kind: execution')
            print(f"  thread: {row['thread_id']}  turn: {row['turn_id']}")
            print(f"  attempted: {facts['attempted']}  dispatched: {facts['dispatched']}  process started: {facts['process_started']}  process exit: {facts['process_exit_code']}")
            print(f"  target input: {facts['target_input_confirmed']}  turn started: {facts['turn_started']}  completed at: {facts['completed_at']}")
            print(f"  instruction SHA-256: {row['instruction_sha256']}")
            print(f"  source: {row['source_path']}  model use: unknown")
            for diagnostic in row['observation_issues']:
                print(f"  observation: {diagnostic['code']}: {diagnostic['description']}")
            if row.get('output'):
                print('  output: ' + canonical(row['output']))
    elif isinstance(value, dict) and 'facts' in value:
        emit([value], False)
    else:
        print(json.dumps(value, ensure_ascii=False, indent=2))


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    extra = []
    if '--' in argv:
        index = argv.index('--')
        argv, extra = argv[:index], argv[index + 1:]
    a = parser().parse_args(argv)
    store = Store(a.store)
    try:
        if extra and a.cmd != 'run':
            raise ValueError('passthrough arguments require run')
        code = 0
        if a.cmd == 'run':
            if a.engine == 'claude' and any(v.split('=', 1)[0] in ('--socket', '--codex') for v in argv):
                raise ValueError('Codex-only option is unsupported for Claude')
            if a.engine == 'agy' and any(v.split('=', 1)[0] in ('--socket', '--codex', '--claude') for v in argv):
                raise ValueError('other-engine option is unsupported for agy')
            output = a.output
            if a.name and ('/' in a.name or a.name in ('.', '..')):
                raise ValueError('name must be a single filename component')
            if output and (a.name or output.is_dir()):
                output = output / ((a.name or 'last-message') + '.md')
            with capture.child_stderr_file(store, a.child_stderr, a.via) as child_stderr:
                attempt = capture.prepare(store, a.instruction, via=a.via, resume=a.resume, thread=a.thread,
                                          socket=a.socket, codex=a.codex, model=a.model, effort=a.effort,
                                          cwd=a.cwd, extra=extra, retry_of=a.retry_of, name=a.name,
                                          output=output, session_root=a.session_root, observation_timeout=a.observation_timeout, engine=a.engine, claude=a.claude, agy=a.agy)
                # A durable ID is available even if the wrapper is killed immediately.
                print('attempt_id=' + attempt, file=sys.stderr, flush=True)
                code = capture.execute(store, attempt, child_stderr=child_stderr)
            result = query.inspect(store, attempt)
            if code == 0 and result['facts']['turn_status'] != 'completed':
                code = 2
        elif a.cmd == 'list':
            result = query.records(store, a.engine)
        elif a.cmd == 'inspect':
            result = query.lookup(store, a.selector, a.engine)
        elif a.cmd == 'dup':
            result = query.duplicates(store, a.selector, a.engine)
        elif a.cmd == 'which':
            no_sensitive(a.selector)
            result = query.which(store, a.selector, a.engine)
        elif a.cmd == 'recover':
            capture.recover(store, a.selector, output=a.output, observation_timeout=a.observation_timeout)
            result = query.inspect(store, a.selector)
            if result['engine'] == 'agy' and result['facts']['turn_status'] == 'unknown':
                code = 2
        elif a.cmd == 'forget':
            store.forget(a.selector)
            result = {'forgotten': a.selector, 'native_records_deleted': False}
        elif a.cmd == 'verify':
            ids = [a.selector] if a.selector else store.ids()
            result = {'verified': [query.inspect(store, aid)['attempt_id'] for aid in ids],
                      'scope': 'stored bytes and interpretation; no native reads or authenticity claim'}
        elif a.cmd == 'export':
            ids = [a.selector] if a.selector else store.ids()
            result = (export.jsonl(store, ids, a.output) if a.format == 'jsonl' else
                      export.polylogue(store, ids, a.output, a.polylogue_bridge))
        if a.cmd in ('run', 'recover'):
            for diagnostic in result.get('observation_issues', []):
                print(canonical({'warning': 'target_observation', **diagnostic}), file=sys.stderr)
            if (result.get('output') or {}).get('status') == 'not_written':
                print(canonical({'warning': 'output_unavailable', 'reason': result['output']['reason']}), file=sys.stderr)
        emit(result, a.json)
        return code if 0 <= code <= 255 else min(255, 128 + abs(code)) if code < 0 else 1
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, IndexError, AttributeError) as error:
        # Exception messages from native tools/JSON can contain arbitrary private bytes.
        message = str(error) if isinstance(error, ValueError) else type(error).__name__
        try:
            no_sensitive(message)
        except ValueError:
            message = 'private diagnostic suppressed'
        emit({'error': message}, getattr(a, 'json', False))
        return 2
    finally:
        for warning in store.warnings:
            print(canonical(warning), file=sys.stderr)
