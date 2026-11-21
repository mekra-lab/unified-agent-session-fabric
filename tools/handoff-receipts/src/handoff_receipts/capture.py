"""One explicit dispatch; durable intent precedes process/RPC mutation."""
import json
import os
import re
import selectors
import signal
import subprocess
import tempfile
import time
from contextlib import contextmanager
from pathlib import Path
from .common import canonical, digest, no_sensitive, now, status, write_bytes, private_dir, dispatch_facts
from .native import read_native, read_snapshot, turn_list, wait_exact
from .rollout import segments, select_segment, observation
from .rpc import Rpc
from .diagnostics import ObservationError, issue, RETRYABLE
from .reported import summarize as stream_summary

DEFAULT_OBSERVATION_TIMEOUT = 2.0


class DispatchRefused(ValueError):
    """Only fixed, public preflight diagnostics belong in persisted reasons."""


def default_socket():
    return Path.home() / '.codex/app-server-control/app-server-control.sock'


def identifier(value):
    if not isinstance(value, str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,100}', value):
        raise ValueError('invalid native identifier')
    no_sensitive(value)
    return value


def session_paths(root):
    return sorted(Path(root).glob('**/rollout-*.jsonl'))


def locate(root, thread):
    identifier(thread)
    matches = [p for p in session_paths(root) if p.name.endswith('-' + thread + '.jsonl')]
    if len(matches) > 1:
        raise ObservationError('ambiguous_rollout_path')
    return matches[0] if matches else None


def command(codex, resume=None, model=None, effort=None, cwd=None, extra=()):
    # Owned transport/prompt/output options cannot be overridden by passthrough.
    forbidden = {'--json', '--output-last-message', '-o', '--ephemeral', '--last', '--all', '--cd', '-C'}
    if any(v.split('=', 1)[0] in forbidden or v == '-' or
           (v.startswith(('-o', '-C')) and not v.startswith('--')) for v in extra):
        raise ValueError('passthrough overrides receipt-owned options')
    args = [str(codex), 'exec'] + (['resume', identifier(resume)] if resume else [])
    if not resume:
        args += ['-C', str(cwd)]
    if model:
        args += ['-m', model]
    if effort:
        args += ['-c', 'model_reasoning_effort=' + effort]
    args += ['--json', '--skip-git-repo-check', *extra, '-']
    no_sensitive(canonical(args))
    return args


def prepare(store, instruction, *, via='exec', resume=None, thread=None, socket=None,
            codex='codex', model=None, effort=None, cwd=None, extra=(), retry_of=None,
            name=None, output=None, session_root=None, observation_timeout=DEFAULT_OBSERVATION_TIMEOUT,
            engine="codex", claude="claude", agy="agy"):
    observation_timeout = validate_timeout(observation_timeout)
    cwd = str(Path(cwd or Path.cwd()).resolve())
    if engine not in ('codex', 'claude', 'agy'):
        raise ValueError('unsupported engine')
    if engine in ('claude', 'agy'):
        from . import claude as adapter
        executable = claude
        if engine == 'agy':
            from . import agy as adapter
            executable = agy
        if via != 'exec' or (thread is not None if engine == 'agy' else thread):
            label = 'Claude' if engine == 'claude' else 'agy'
            raise ValueError(label + ' supports exec only; use --resume for a session')
        # Validate UTF-8 before any engine-specific native preflight.
        if engine == 'agy':
            try:
                Path(instruction).read_bytes().decode('utf-8')
            except UnicodeError:
                raise ValueError('agy instruction must be UTF-8; no dispatch') from None
        fields = adapter.request_fields(executable, resume, model, effort, cwd, session_root, extra)
        request = {'engine': engine, 'interpretation_profile': adapter.PROFILE, 'via': 'exec',
                   'observation_timeout': observation_timeout, 'cwd': cwd, 'model': model, 'effort': effort,
                   'output': str(Path(output).absolute()) if output else None, 'socket': None, **fields}
        if output and Path(output).absolute().is_relative_to(store.root):
            raise ValueError('final output must be outside the receipt store')
        body = Path(instruction).read_bytes()
        if len(body) > 1024 * 1024:
            raise ValueError('instruction exceeds 1 MiB capture limit')
        adapter.private_check(body.decode())
        adapter.private_check(canonical(request))
        return store.create(instruction, request, retry_of, name)
    session_root = str(Path(session_root or Path(os.environ.get('CODEX_HOME', Path.home() / '.codex')) / 'sessions').absolute())
    socket = (socket or default_socket()) if via == 'daemon' else None
    if via == 'daemon' and (not thread or resume or extra):
        raise ValueError('daemon requires --thread; resume/passthrough are exec-only')
    if via == 'exec' and thread:
        raise ValueError('use --resume for exec')
    target_thread = identifier(resume or thread) if resume or thread else None
    path = locate(session_root, target_thread) if target_thread and via == 'exec' else None
    preflight = None
    if path:
        before = read_snapshot(path)
        preflight = {'bytes': len(before), 'sha256': digest(before)}
    if resume and not path:
        raise ValueError('resume rollout not found; cannot establish pre-dispatch boundary')
    if output and Path(output).absolute().is_relative_to(store.root):
        raise ValueError('final output must be outside the receipt store')
    request = {'engine': 'codex', 'interpretation_profile': 'codex-selected-lines-v2', 'observation_timeout': observation_timeout, 'via': via, 'thread_id': target_thread, 'cwd': cwd, 'session_root': session_root,
               'native_path': str(path) if path else None, 'preflight': preflight,
               'model': model, 'effort': effort, 'output': str(Path(output).absolute()) if output else None,
               'socket': str(Path(socket).absolute()) if socket else None,
               'known_paths': [digest(str(p)) for p in session_paths(session_root)] if not target_thread else [],
               'argv': command(codex, resume, model, effort, cwd, extra) if via == 'exec' else None}
    if len(Path(instruction).read_bytes()) > 1024 * 1024:
        raise ValueError('instruction exceeds 1 MiB capture limit')
    return store.create(instruction, request, retry_of, name)


def add_event(store, attempt, state, event, **fields):
    state['control'].append({'event': event, 'at': now(), **fields})
    store.save(attempt, state)


def validate_timeout(value):
    value = float(value)
    if not 0 <= value <= 30:
        raise ValueError('observation timeout must be between 0 and 30 seconds')
    return value


def _collect_once(request, state, body):
    if request.get('engine') == 'agy':
        from .agy import collect_once
        return collect_once(request, state, body)
    if request.get('engine', 'codex') == 'claude':
        from .claude import collect_once
        return collect_once(request, state, body)
    target = state['target']
    thread = target.get('thread_id') or request['thread_id']
    path = target.get('native_path') or request['native_path']
    preflight = target.get('preflight') or request.get('preflight')
    minimum = (preflight or {}).get('bytes', 0)
    candidates, diagnostics = [], []
    try:
        if thread:
            path = Path(path) if path else locate(request['session_root'], thread)
            paths = [(path, thread)] if path else []
        else:
            paths = []
            known = set(request['known_paths'])
            for item in session_paths(request['session_root']):
                if digest(str(item)) in known:
                    continue
                match = re.search(r'([0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})\.jsonl$', item.name)
                if match:
                    paths.append((item, match[1]))
        if not paths:
            raise ObservationError('rollout_not_found')
        for path, tid in paths:
            content, parsed = read_native(path, tid)
            if preflight and (len(content) < minimum or digest(content[:minimum]) != preflight['sha256']):
                raise ObservationError('preflight_prefix_changed', preflight_bytes=minimum, snapshot_bytes=len(content))
            if not thread and parsed[0][2]['payload'].get('cwd') != request['cwd']:
                continue
            for seg in segments(content, parsed, body, minimum, target.get('turn_id')):
                if target.get('turn_id') and target['turn_id'] != seg['turn_id']:
                    continue
                selected, copied = select_segment(content, parsed, seg, body)
                identifier(seg['turn_id'])
                selected.update(copy_text=copied.decode('utf-8'), path=str(path), observed_at=now())
                no_sensitive(canonical(selected))
                candidates.append((tid, str(path), selected))
        if len(candidates) == 1:
            tid, path, selected = candidates[0]
            bound = target.get('turn_id') == selected['turn_id'] or selected['matches']
            if bound:
                target.update(thread_id=tid, turn_id=selected['turn_id'], native_path=path, segments=[selected])
                omitted = [r for r in selected['omitted'] if r['reason'] == 'sensitive' and r['roles']]
                if omitted:
                    diagnostics.append(issue('sensitive_required_lines', line_count=len(omitted)))
                if not selected['matches']:
                    diagnostics.append(issue('target_input_missing'))
                if not selected['terminal']:
                    diagnostics.append(issue('turn_terminal_missing'))
                elif selected['terminal'] == 'completed' and not selected['final_output_sha256']:
                    diagnostics.append(issue('final_output_missing'))
            else:
                diagnostics.append(issue('input_binding_omitted'))
        else:
            diagnostics.append(issue('ambiguous_target_match' if candidates else 'no_matching_turn',
                                     candidate_count=len(candidates), preflight_bytes=minimum))
    except ObservationError as error:
        diagnostics.append(error.diagnostic)
    except FileNotFoundError:
        diagnostics.append(issue('rollout_not_found'))
    except OSError:
        diagnostics.append(issue('rollout_unreadable'))
    except (ValueError, KeyError, TypeError, AttributeError):
        diagnostics.append(issue('native_selection_invalid'))
    target['diagnostics'] = diagnostics
    target['issues'] = [d['code'] for d in diagnostics]


def collect(store, attempt, state, timeout=None):
    """Bounded rereads for late flushes. No resend and no live-source query path."""
    _, request, _, body = store.load(attempt)
    if not dispatch_facts(request['via'], state['control'])['attempted']:
        return state
    budget = validate_timeout(request.get('observation_timeout', DEFAULT_OBSERVATION_TIMEOUT) if timeout is None else timeout)
    deadline = time.monotonic() + budget
    attempts = []
    while True:
        _collect_once(request, state, body)
        codes = state['target']['issues']
        attempts.append({'at': now(), 'codes': list(codes)})
        remaining = deadline - time.monotonic()
        retry_codes = codes
        if request.get('engine') == 'agy' and 'agy_executor_pending' in codes:
            retry_codes = [code for code in codes if code != 'agy_fields_omitted']
        # Sensitive or ambiguous evidence will not become attributable by waiting.
        if not retry_codes or any(code not in RETRYABLE for code in retry_codes) or remaining <= 0:
            break
        time.sleep(min(.1, remaining))
    state['target']['observation_attempts'] = attempts
    state['target']['observation_timeout'] = budget
    store.save(attempt, state)
    return state


def output_message(store, attempt, state, output=None):
    _, request, _, body = store.load(attempt)
    destination = output or request['output']
    if not destination:
        return
    text, level = None, None
    path = Path(destination).absolute()
    no_sensitive(str(path))
    if path.is_relative_to(store.root):
        raise ValueError('final output must be outside the receipt store')
    if request.get('engine', 'codex') in ('claude', 'agy'):
        from .claude import reported, private_check
        if request['engine'] == 'agy':
            from .agy import reported
        from .query import inspect
        receipt = inspect(store, attempt)
        report = reported(request, state['control'])
        if receipt['facts']['turn_status'] == 'completed':
            text, level = report['final_message'], 'reported'
        if text is not None:
            private_check(text)
            if not path.parent.exists():
                private_dir(path.parent)
            write_bytes(path, text.encode())
            add_event(store, attempt, state, 'output_written', sha256=digest(text), assessment=level,
                      source=request['engine'] + '_json_result', path=str(path))
        else:
            add_event(store, attempt, state, 'output_unavailable', reason='no_safe_completed_result')
        return
    for seg in state['target']['segments']:
        facts, parsed = observation(seg, seg['copy_text'].encode(), body)
        if not facts['final_output_sha256']:
            continue
        for _, _, row in parsed:
            p = row.get('payload', {})
            candidate = p.get('last_agent_message')
            if p.get('role') == 'assistant' and p.get('phase') == 'final_answer':
                candidate = ''.join(v.get('text', '') for v in p.get('content', [])
                                    if v.get('type') in ('text', 'input_text', 'output_text'))
            if isinstance(candidate, str) and digest(candidate) == facts['final_output_sha256']:
                text, level = candidate, 'observed'
    if text is None:
        reported = stream_summary(request, state['control'])
        text = reported['final_message']
        level = 'reported' if text is not None else None
    if text is not None:
        no_sensitive(text)
        if not path.parent.exists():
            private_dir(path.parent)
        write_bytes(path, text.encode())
        add_event(store, attempt, state, 'output_written', sha256=digest(text), assessment=level,
                  source='target_rollout' if level == 'observed' else 'exec_json_stream', path=str(path))
    else:
        add_event(store, attempt, state, 'output_unavailable', reason='no_safe_final_message')


@contextmanager
def child_stderr_file(store, path, via='exec'):
    """Open caller-managed diagnostics before creating any attempt."""
    if path is None:
        yield subprocess.DEVNULL
        return
    if via != 'exec':
        raise ValueError('--child-stderr requires --via exec')
    path = Path(path).absolute()
    if path.resolve().is_relative_to(store.root):
        raise ValueError('child stderr must be outside the receipt store')
    if not os.access(path.parent, os.W_OK):
        raise ValueError('child stderr parent directory must exist and be writable')
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'wb') as stderr:
        os.fchmod(fd, 0o600)
        yield stderr


def execute(store, attempt, *, child_stderr=subprocess.DEVNULL):
    with store.lock(attempt):
        _, request, state, _ = store.load(attempt)
        if any(e['event'] == 'dispatch_started' for e in state['control']):
            raise ValueError('attempt already dispatched; use an explicit new retry')
        if request['via'] == 'daemon':
            return daemon(store, attempt, request, state)
        is_agy = request.get('engine') == 'agy'
        if is_agy:
            from . import agy
            if request['thread_id']:
                try:
                    agy.read_database(request['native_path'], request['thread_id'], request['preflight'], boundary_only=True)
                except (OSError, ValueError, TypeError):
                    add_event(store, attempt, state, 'dispatch_rejected', reason='agy resume preflight changed or unavailable; no dispatch')
                    return 2
            add_event(store, attempt, state, 'agy_version_before', **agy.fingerprint(request))
        add_event(store, attempt, state, 'dispatch_started')
        proc, caught, handlers = None, [], {}
        try:
            with (tempfile.TemporaryFile() if is_agy else (store.path(attempt) / 'body.txt').open('rb')) as stdin:
                if is_agy:
                    body = store.load(attempt)[3].decode('utf-8')
                    stdin.write((canonical({'event': 'user', 'message': {'content': body}}) + '\n').encode('utf-8'))
                    stdin.seek(0)
                proc = subprocess.Popen(request['argv'], cwd=request['cwd'], stdin=stdin,
                                        stdout=subprocess.PIPE, stderr=child_stderr, start_new_session=True)
            add_event(store, attempt, state, 'process_started', pid=proc.pid)
            def forward(signum, frame):
                caught.append(signum)
                try:
                    os.killpg(proc.pid, signum)
                except ProcessLookupError:
                    pass
            for sig in (signal.SIGINT, signal.SIGTERM):
                handlers[sig] = signal.signal(sig, forward)
            pending = b''
            discarding = False
            selector = selectors.DefaultSelector()
            def consume(line):
                try:
                    event = json.loads(line)
                    if is_agy:
                        for name, fields in agy.stream_events(event):
                            if name == 'agy_identity' and not state['target'].get('thread_id'):
                                state['target']['thread_id'] = fields['conversation_id']
                            add_event(store, attempt, state, name, **fields)
                        return
                    if request.get('engine', 'codex') == 'claude':
                        if not isinstance(event, dict):
                            raise ValueError('unsupported Claude stream shape')
                        from .claude import stream_events
                        for name, fields in stream_events(event, request['thread_id']):
                            add_event(store, attempt, state, name, **fields)
                        return
                    if not isinstance(event, dict):
                        raise ValueError('unsupported Codex stream shape')
                    kind = event.get('type')
                    # Retain allowlisted control fields and privacy-checked final-message
                    # candidates only; never raw events, stderr or account metadata.
                    if kind == 'thread.started':
                        tid = identifier(event['thread_id'])
                        if request['thread_id'] and request['thread_id'] != tid:
                            add_event(store, attempt, state, 'stream_identity_mismatch')
                            return
                        state['target']['thread_id'] = tid
                        add_event(store, attempt, state, 'stream_thread', thread_id=tid)
                    elif kind in ('turn.started', 'turn.completed', 'turn.failed', 'error'):
                        fields = {}
                        if event.get('turn_id'):
                            fields['turn_id'] = identifier(event['turn_id'])
                            if state['target'].get('turn_id') not in (None, fields['turn_id']):
                                add_event(store, attempt, state, 'stream_identity_mismatch')
                                return
                            state['target']['turn_id'] = fields['turn_id']
                        add_event(store, attempt, state, 'stream_' + kind.replace('.', '_'), **fields)
                    elif kind == 'item.completed' and isinstance(event.get('item'), dict):
                        item = event['item']
                        if item.get('type') == 'agent_message' and isinstance(item.get('text'), str):
                            try:
                                no_sensitive(item['text'])
                            except ValueError:
                                add_event(store, attempt, state, 'stream_final_message_omitted', reason='sensitive')
                            else:
                                add_event(store, attempt, state, 'stream_final_message',
                                          text=item['text'], sha256=digest(item['text']))
                except (ValueError, KeyError, TypeError, RecursionError):
                    add_event(store, attempt, state, 'stream_unrecognized')
                    if request.get('engine', 'codex') == 'claude':
                        add_event(store, attempt, state, 'claude_stream_incomplete')
                    if is_agy:
                        add_event(store, attempt, state, 'agy_stream_incomplete')
            try:
                selector.register(proc.stdout, selectors.EVENT_READ)
                while selector.get_map():
                    for key, _ in selector.select(.2):
                        chunk = os.read(key.fileobj.fileno(), 65536)
                        if not chunk:
                            selector.unregister(key.fileobj)
                            if pending and not discarding:
                                consume(pending)
                            break
                        if discarding:
                            if b'\n' not in chunk:
                                continue
                            _, chunk = chunk.split(b'\n', 1)
                            discarding = False
                        pending += chunk
                        while b'\n' in pending:
                            line, pending = pending.split(b'\n', 1)
                            if len(line) <= 4 * 1024 * 1024:
                                consume(line)
                            else:
                                add_event(store, attempt, state, 'stream_record_too_large')
                                if request.get('engine', 'codex') == 'claude':
                                    add_event(store, attempt, state, 'claude_stream_incomplete')
                                if is_agy:
                                    add_event(store, attempt, state, 'agy_stream_incomplete')
                                add_event(store, attempt, state, 'stream_final_message_omitted', reason='size_limit')
                        if len(pending) > 4 * 1024 * 1024:
                            pending = b''
                            discarding = True
                            add_event(store, attempt, state, 'stream_record_too_large')
                            if request.get('engine', 'codex') == 'claude':
                                add_event(store, attempt, state, 'claude_stream_incomplete')
                            if is_agy:
                                add_event(store, attempt, state, 'agy_stream_incomplete')
                            # Its type is unavailable; do not reuse an earlier final candidate.
                            add_event(store, attempt, state, 'stream_final_message_omitted', reason='size_limit')
                code = proc.wait()
            finally:
                selector.close()
                proc.stdout.close()
            for sig in caught:
                add_event(store, attempt, state, 'signal_forwarded', signal=sig)
            add_event(store, attempt, state, 'process_exited', exit_code=code)
        except OSError as error:
            add_event(store, attempt, state, 'process_error', category=type(error).__name__)
            if proc is not None:
                if request.get('engine') in ('claude', 'agy'):
                    add_event(store, attempt, state, request['engine'] + '_stream_incomplete')
                # Children run in their own session, as in signal forwarding above.
                if proc.poll() is None:
                    try:
                        os.killpg(proc.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                try:
                    exit_code = proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(proc.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    exit_code = proc.wait()
                add_event(store, attempt, state, 'process_exited', exit_code=exit_code)
            code = 127
        finally:
            for sig, handler in handlers.items():
                signal.signal(sig, handler)
            if is_agy:
                add_event(store, attempt, state, 'agy_version_after', **agy.fingerprint(request))
        collect(store, attempt, state)
        output_message(store, attempt, state)
        if is_agy:
            from .query import inspect
            outcome = inspect(store, attempt)['facts']['turn_status']
            if outcome == 'unknown' or (code == 0 and outcome != 'completed'):
                return 2
        return code


class LoggedRpc(Rpc):
    """Persist the request BEFORE send; retain only allowlisted response fields."""
    def __init__(self, socket, store, attempt, state):
        self.store, self.attempt, self.state = store, attempt, state
        super().__init__(socket, name='handoff_receipts')

    def call(self, method, params=None):
        add_event(self.store, self.attempt, self.state, 'rpc_request', method=method,
                  request_id=self.counter + 1, params=params or {})
        try:
            value = super().call(method, params)
        except Exception as error:
            add_event(self.store, self.attempt, self.state, 'rpc_error', method=method,
                      category=type(error).__name__)
            raise
        fields = {}
        if method == 'turn/start':
            fields['turn_id'] = identifier(value['turn']['id'])
        add_event(self.store, self.attempt, self.state, 'rpc_response', method=method, **fields)
        return value


def daemon(store, attempt, request, state):
    client = None
    code = 2
    try:
        thread = request['thread_id']
        client = LoggedRpc(request['socket'], store, attempt, state)
        info = client.call('thread/read', {'threadId': thread})['thread']
        if info['id'] != thread:
            raise DispatchRefused('daemon thread ID mismatch')
        if status(info.get('status')) == 'notLoaded':
            raise DispatchRefused('thread not loaded in the daemon; open it in the Codex app or use --via exec --resume')
        if status(info.get('status')) != 'idle':
            raise DispatchRefused('daemon target must be idle before dispatch')
        prior = turn_list(client, thread)  # not materialized means no prior turns
        path = info.get('path')
        try:
            if not path:
                raise FileNotFoundError
            before = read_snapshot(path)
        except FileNotFoundError:
            if prior:
                raise DispatchRefused('daemon rollout unavailable for an existing thread; no dispatch') from None
            before = b''  # Fresh idle thread: file/path appears on its first turn.
        state['target'].update(thread_id=thread, native_path=path,
                               preflight={'bytes': len(before), 'sha256': digest(before)},
                               prior_turn_ids=[t['id'] for t in prior])
        add_event(store, attempt, state, 'dispatch_started')
        body = store.load(attempt)[3].decode()
        params = {'threadId': thread, 'input': [{'type': 'text', 'text': body}]}
        if request['model']:
            params['model'] = request['model']
        if request['effort']:
            params['effort'] = request['effort']
        turn = client.call('turn/start', params)['turn']['id']
        state['target']['turn_id'] = identifier(turn)
        store.save(attempt, state)
        exact = wait_exact(client, thread, turn, 300)
        state['target']['exact_status'] = exact
        code = 0 if exact and exact['status'] == 'completed' else 2
    except Exception as error:
        if not dispatch_facts('daemon', state['control'])['attempted']:
            reason = str(error) if isinstance(error, DispatchRefused) else 'daemon preflight failed: ' + type(error).__name__
            add_event(store, attempt, state, 'dispatch_rejected', reason=reason)
        add_event(store, attempt, state, 'daemon_error', category=type(error).__name__)
    finally:
        if client:
            if dispatch_facts('daemon', state['control'])['attempted']:
                try:
                    info = client.call('thread/read', {'threadId': request['thread_id']})['thread']
                    if info['id'] == request['thread_id'] and info.get('path'):
                        no_sensitive(info['path'])
                        state['target']['native_path'] = info['path']
                        store.save(attempt, state)
                except (OSError, ValueError, RuntimeError, KeyError, TypeError):
                    pass  # A lost connection must never trigger another turn/start.
            client.close()
        collect(store, attempt, state)
        output_message(store, attempt, state)
    return code


def recover(store, attempt, *, output=None, observation_timeout=None):
    with store.lock(attempt):
        _, request, state, _ = store.load(attempt)
        add_event(store, attempt, state, 'recovery_started')
        if request.get('engine') == 'agy':
            from . import agy
            add_event(store, attempt, state, 'agy_version_recover', **agy.fingerprint(request))
        collect(store, attempt, state, timeout=observation_timeout)
        output_message(store, attempt, state, output=output)
    return state
