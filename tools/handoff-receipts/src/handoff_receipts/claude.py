"""Claude headless profile: explicit session binding and selective evidence only."""
import json
import re
import uuid
from pathlib import Path
from .common import canonical, digest, no_sensitive, now
from .diagnostics import ObservationError, issue
from .native import read_snapshot

PROFILE = 'claude-selected-lines-v1'
UNKNOWN = 'unknown'
INTERRUPTED = '[Request interrupted by user]'


def session_id(value):
    try:
        parsed = str(uuid.UUID(value))
    except (ValueError, TypeError, AttributeError):
        raise ValueError('Claude session ID must be a UUID') from None
    if parsed != value.lower():
        raise ValueError('Claude session ID must be a canonical UUID')
    return parsed


def private_check(text):
    no_sensitive(text)
    # Claude adds organization and authentication fields not used by Codex's
    # existing scanner. Keep this stricter rule confined to this engine profile.
    if re.search(r'(?i)(?:(?:account|organization)(?:Uuid|_id|Id)?|org_id|userId|userUuid|userEmail|apiKeySource|apiKey|credential|oauth|authToken)\s*["\x27:=]', text):
        raise ValueError('sensitive Claude content refused before persistence')


# Known flag arities make positional prompts/abbreviations and short-option
# bundles unambiguous. Unsupported options fail closed before any subprocess.
EXTRA_FLAGS = {'--safe-mode', '--disable-slash-commands', '--strict-mcp-config',
               '--no-chrome', '--include-partial-messages'}
EXTRA_VALUES = {'--tools', '--allowedTools', '--allowed-tools', '--disallowedTools',
                '--disallowed-tools', '--permission-mode', '--permission-prompts',
                '--max-budget-usd', '--append-system-prompt', '--system-prompt',
                '--setting-sources'}


def extra_arguments(extra):
    result, i = [], 0
    while i < len(extra):
        arg = extra[i]
        key, eq, value = arg.partition('=')
        if key in EXTRA_FLAGS and not eq:
            result.append(arg)
        elif key in EXTRA_VALUES:
            if not eq:
                i += 1
                if i >= len(extra) or extra[i].startswith('-'):
                    raise ValueError('Claude passthrough option requires one value')
                value = extra[i]
            private_check(value)
            result += [key, value]
        else:
            raise ValueError('unsupported or wrapper-owned Claude passthrough argument')
        i += 1
    return result


def request_fields(executable, resume, model, effort, cwd, projects_root, extra):
    if effort is not None and effort not in ('low', 'medium', 'high', 'xhigh', 'max'):
        raise ValueError('unsupported Claude effort')
    extras = extra_arguments(extra)
    sid = session_id(resume) if resume else str(uuid.uuid4())
    root = Path(projects_root or Path.home() / '.claude/projects').absolute()
    project = root / str(Path(cwd)).replace('/', '-')
    path = project / (sid + '.jsonl')
    if path.absolute() != path.resolve():
        raise ValueError('unsafe Claude native path')
    if resume:
        try:
            raw = read_snapshot(path)
        except FileNotFoundError:
            raw = None  # No prefix boundary: dispatch reports may exist, native facts stay unknown.
    else:
        if path.exists():
            raise ValueError('Claude session ID already exists in the selected project')
        raw = b''
    args = [str(executable), '-p', '--output-format', 'stream-json', '--verbose',
            '--resume' if resume else '--session-id', sid]
    if model:
        args += ['--model', model]
    if effort:
        args += ['--effort', effort]
    # The wrapper's -- separates its options from passthrough. Forward flags as
    # flags, not after Claude's own end-of-options delimiter (which means prompt).
    args += extras
    private_check(canonical(args))
    return {'thread_id': sid, 'session_root': str(root), 'project_path': str(project),
            'native_path': str(path), 'preflight': {'bytes': len(raw), 'sha256': digest(raw)} if raw is not None else None,
            'known_paths': [], 'argv': args}


def stream_events(event, expected):
    """Yield only fixed-shape, sanitized control facts, never raw SDK payloads."""
    kind = event.get('type')
    if kind != 'result' and not (kind == 'system' and event.get('subtype') == 'init'):
        return []
    if event.get('session_id') != expected:
        return [('claude_identity_mismatch', {})]
    if kind == 'system':
        return [('claude_init', {'session_id': expected})]
    reason = event.get('terminal_reason')
    reason = reason if reason in ('completed', 'api_error', 'aborted_streaming', 'interrupted',
                                  'aborted', 'cancelled', 'error', 'max_turns', 'max_budget') else UNKNOWN
    fields = {'session_id': expected, 'is_error': event.get('is_error') if isinstance(event.get('is_error'), bool) else None,
              'terminal_reason': reason}
    text = event.get('result')
    if isinstance(text, str) and fields['is_error'] is False and reason == 'completed':
        try:
            private_check(text)
        except ValueError:
            fields['output_omitted'] = 'sensitive'
        else:
            fields.update(text=text, sha256=digest(text))
    return [('claude_result', fields)]


def reported(request, control):
    result = {'thread_id': UNKNOWN, 'turn_id': UNKNOWN, 'turn_started': UNKNOWN,
              'turn_status': UNKNOWN, 'final_output_sha256': UNKNOWN, 'final_message': None}
    if any(e['event'] in ('claude_identity_mismatch', 'claude_stream_incomplete') for e in control):
        return result
    events = [e for e in control if e['event'] in ('claude_init', 'claude_result')]
    if any(e.get('session_id') != request['thread_id'] for e in events):
        return result
    if events:
        result['thread_id'] = request['thread_id']
    terminals = [e for e in events if e['event'] == 'claude_result']
    if len(terminals) != 1:
        return result
    event = terminals[0]
    reason = event.get('terminal_reason')
    if event.get('is_error') is False and reason == 'completed':
        result['turn_status'] = 'completed'
    elif event.get('is_error') is True:
        result['turn_status'] = 'interrupted' if reason in ('aborted_streaming', 'interrupted', 'aborted', 'cancelled') else 'failed'
    result['reported_at'] = event.get('at')
    if result['turn_status'] == 'completed' and isinstance(event.get('text'), str):
        private_check(event['text'])
        if digest(event['text']) != event.get('sha256'):
            raise ValueError('Claude stream output hash mismatch')
        result.update(final_message=event['text'], final_output_sha256=event['sha256'])
    return result


def message_text(row):
    msg = row.get('message')
    if not isinstance(msg, dict):
        return None
    content = msg.get('content')
    if isinstance(content, str):
        return content
    if isinstance(content, list) and all(isinstance(v, dict) for v in content):
        texts = [v.get('text') for v in content if v.get('type') == 'text']
        if all(isinstance(v, str) for v in texts):
            return ''.join(texts)
    return None


def input_user(row):
    msg = row.get('message', {})
    content = msg.get('content') if isinstance(msg, dict) else None
    return (row.get('type') == 'user' and isinstance(msg, dict) and msg.get('role') == 'user'
            and isinstance(row.get('uuid'), str) and message_text(row) != INTERRUPTED
            and not any(row.get(k) for k in ('isMeta', 'isCompactSummary', 'isVisibleInTranscriptOnly'))
            and (isinstance(content, str) or (isinstance(content, list) and bool(content)
                 and all(isinstance(v, dict) and v.get('type') == 'text' for v in content))))


def roles(row, turn, prompt_id, body):
    if input_user(row) and row['uuid'] == turn and message_text(row).encode() == body:
        return ['input']
    if not prompt_id or row.get('promptId') != prompt_id:
        return []
    if (row.get('type') == 'assistant' and isinstance(row.get('uuid'), str)
            and isinstance(row.get('message'), dict) and row['message'].get('role') == 'assistant'
            and message_text(row) is not None):
        return ['assistant']
    if row.get('type') == 'user' and message_text(row) == INTERRUPTED:
        return ['interruption']
    return []



def connection_graph(rows, turn):
    """Exclude other actual inputs; duplicate IDs cannot supply a graph node."""
    graph, seen = {}, set()
    for row in rows:
        key = row.get('uuid')
        if not isinstance(key, str):
            continue
        if key in seen:
            graph.pop(key, None)
            continue
        seen.add(key)
        if not re.fullmatch(r'[A-Za-z0-9_-]{1,100}', key):
            continue
        parent = row.get('parentUuid')
        if parent is not None and (not isinstance(parent, str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,100}', parent)):
            continue
        if input_user(row) and key != turn:
            continue
        graph[key] = parent
    return graph


def connection_path(row, turn, graph):
    key, seen, path = row.get('uuid'), set(), []
    if not isinstance(key, str):
        return []
    while key != turn:
        if key in seen or key not in graph:
            return []
        seen.add(key);path.append(key)
        key = graph[key]
    return path if turn in graph else []


def linked_roles(row, turn, prompt_id, body, graph):
    if input_user(row) and row['uuid'] == turn and message_text(row).encode() == body:
        return ['input']
    # An explicitly conflicting prompt ID cannot be repaired by a parent link.
    if row.get('promptId') is not None and row.get('promptId') != prompt_id:
        return []
    if not connection_path(row, turn, graph):
        return []
    if (row.get('type') == 'assistant' and isinstance(row.get('message'), dict)
            and row['message'].get('role') == 'assistant' and message_text(row) is not None):
        return ['assistant']
    if row.get('type') == 'user' and message_text(row) == INTERRUPTED:
        return ['interruption']
    return []

def read_rows(path, expected):
    raw = read_snapshot(path)
    raw = raw[:raw.rfind(b'\n') + 1]
    parsed, offset = [], 0
    for number, line in enumerate(raw.splitlines(keepends=True), 1):
        try:
            row = json.loads(line)
        except (ValueError, UnicodeError):
            raise ObservationError('rollout_invalid_json', line=number) from None
        if not isinstance(row, dict):
            raise ObservationError('rollout_invalid_shape', line=number)
        if row.get('sessionId') not in (None, expected):
            raise ObservationError('rollout_thread_mismatch')
        parsed.append((offset, offset + len(line), row))
        offset += len(line)
    if not parsed:
        raise ObservationError('rollout_incomplete')
    return raw, parsed


def select(raw, parsed, request, body):
    pre = request['preflight']
    minimum = pre['bytes']
    if len(raw) < minimum or digest(raw[:minimum]) != pre['sha256']:
        raise ObservationError('preflight_prefix_changed')
    matches = [(a, b, r) for a, b, r in parsed if a >= minimum and input_user(r)
               and r.get('sessionId') == request['thread_id'] and message_text(r).encode() == body]
    if len(matches) != 1:
        raise ObservationError('ambiguous_target_match' if matches else 'no_matching_turn', candidate_count=len(matches))
    start, _, user = matches[0]
    turn, prompt_id = user['uuid'], user.get('promptId')
    # Native IDs must never become an unfiltered diagnostic or source path.
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,100}', turn):
        raise ObservationError('native_selection_invalid')
    end = len(raw)
    graph = connection_graph([r for a, b, r in parsed if a >= start and b <= end], turn)
    needed = set()
    lines, omitted, parts, offset = [], [], [], 0
    input_retained = False
    for a, b, row in parsed:
        if a < start or b > end:
            continue
        label = linked_roles(row, turn, prompt_id, body, graph)
        if label and label != ['input']:
            needed.update(connection_path(row, turn, graph))
        item = {'start': a, 'end': b, 'sha256': digest(raw[a:b]), 'roles': label}
        reason = 'not_required'
        if label and (label == ['input'] or input_retained):
            try:
                private_check(raw[a:b].decode())
                private_check(canonical(row))
            except ValueError:
                reason = 'sensitive'
            else:
                lines.append({**item, 'copy_start': offset, 'copy_end': offset + b - a})
                parts.append(raw[a:b]); offset += b - a
                if label == ['input']:
                    input_retained = True
                continue
        omitted.append({**item, 'reason': reason})
    # Derived projection contains no original payload or record-type metadata.
    by_start = {a: r for a, _, r in parsed}
    projections = []
    for item in omitted:
        row = by_start[item['start']]
        if input_retained and isinstance(row.get('uuid'), str) and row['uuid'] in needed and row['uuid'] in graph:
            projections.append({'kind': 'derived_connection', 'fields': {
                'uuid': row['uuid'], 'parentUuid': row.get('parentUuid')},
                'start': item['start'], 'end': item['end'], 'sha256': item['sha256']})
    copied = b''.join(parts)
    segment = {'format': PROFILE, 'association': 'parent-chain-v1', 'connection_projections': projections, 'turn_id': turn, 'turn_key': {'kind': 'user_message', 'value': turn},
               'start': start, 'end': end, 'snapshot_sha256': digest(raw), 'range_sha256': digest(raw[start:end]),
               'copy_text': copied.decode(), 'copy_sha256': digest(copied), 'lines': lines, 'omitted': omitted,
               'observed_at': now()}
    # promptId is recovered from a retained input, never trusted from a summary.
    return segment


def observation(segment, body, expected):
    if segment.get('format') != PROFILE or segment.get('turn_key') != {'kind': 'user_message', 'value': segment.get('turn_id')}:
        raise ValueError('invalid Claude observation profile or turn key')
    raw = segment['copy_text'].encode()
    if digest(raw) != segment['copy_sha256']:
        raise ValueError('Claude selected copy hash mismatch')
    cursor = segment['start']
    for item in sorted(segment['lines'] + segment['omitted'], key=lambda v: v['start']):
        if item['start'] != cursor or item['end'] <= cursor:
            raise ValueError('Claude inventory gap or overlap')
        cursor = item['end']
    if cursor != segment['end']:
        raise ValueError('Claude inventory span mismatch')
    parsed, offset = [], 0
    for item in segment['lines']:
        if item['copy_start'] != offset or item['copy_end'] - offset != item['end'] - item['start']:
            raise ValueError('Claude selected offset mismatch')
        line = raw[offset:item['copy_end']]
        if not line.endswith(b'\n') or len(line.splitlines()) != 1 or digest(line) != item['sha256']:
            raise ValueError('Claude selected line hash mismatch')
        row = json.loads(line);private_check(line.decode());private_check(canonical(row))
        if row.get('sessionId') != expected:
            raise ValueError('Claude selected session mismatch')
        parsed.append((item, row));offset = item['copy_end']
    if offset != len(raw):
        raise ValueError('Claude unindexed copy bytes')
    inputs = [r for _, r in parsed if input_user(r) and r['uuid'] == segment['turn_id'] and message_text(r).encode() == body]
    prompt_id = inputs[0].get('promptId') if len(inputs) == 1 else None
    graph = None
    if segment.get('association') is not None:
        if segment['association'] != 'parent-chain-v1':
            raise ValueError('unsupported Claude connection interpretation')
        omitted = {i['start']: i for i in segment['omitted']}
        bridges, seen = [], set()
        for projection in segment['connection_projections']:
            if set(projection) != {'kind', 'fields', 'start', 'end', 'sha256'} or projection['kind'] != 'derived_connection':
                raise ValueError('invalid Claude connection projection')
            fields = projection['fields']
            if not isinstance(fields, dict) or set(fields) != {'uuid', 'parentUuid'}:
                raise ValueError('invalid Claude connection fields')
            source = omitted.get(projection['start'])
            if not source or projection['start'] in seen or any(projection[k] != source[k] for k in ('start', 'end', 'sha256')):
                raise ValueError('invalid Claude connection provenance')
            seen.add(projection['start'])
            private_check(canonical(fields))
            bridges.append(fields)
        graph = connection_graph([r for _, r in parsed] + bridges, segment['turn_id'])
        if any(r.get('uuid') not in graph for r in bridges):
            raise ValueError('ambiguous or invalid Claude connection identifiers')
    elif segment.get('connection_projections'):
        raise ValueError('unversioned Claude connection projection')
    traces, assistants = [], []
    for item, row in parsed:
        label = (roles(row, segment['turn_id'], prompt_id, body) if graph is None else
                 linked_roles(row, segment['turn_id'], prompt_id, body, graph))
        if not label or label != item['roles']:
            raise ValueError('Claude selected role or prompt binding mismatch')
        anchor = {'assessment': 'observed', 'line_start': item['start'], 'record_uuid': row.get('uuid'),
                  'timestamp': row.get('timestamp')}
        if label == ['interruption']:
            traces.append({'kind': 'interruption', **anchor})
        if label == ['assistant']:
            msg = row['message'];text = message_text(row)
            synthetic = bool(row.get('isApiErrorMessage')) or msg.get('model') == '<synthetic>'
            if synthetic:
                traces.append({'kind': 'synthetic_error', **anchor})
            if msg.get('stop_reason') == 'end_turn':
                traces.append({'kind': 'message_end', **anchor})
            assistants.append((item['start'], row, text, synthetic))
    auxiliary = {'native_message_end': UNKNOWN, 'native_output_sha256': UNKNOWN, 'native_message_timestamp': UNKNOWN}
    omitted_assistants = [i['start'] for i in segment['omitted'] if 'assistant' in i['roles']]
    if assistants and (not omitted_assistants or max(omitted_assistants) < assistants[-1][0]):
        _, row, text, synthetic = assistants[-1]
        auxiliary['native_message_end'] = row['message'].get('stop_reason') or UNKNOWN
        auxiliary['native_message_timestamp'] = row.get('timestamp') or UNKNOWN
        if isinstance(text, str) and not synthetic:
            auxiliary['native_output_sha256'] = digest(text)
    return {'matches': len(inputs) == 1, 'native_traces': traces, 'auxiliary': auxiliary}


def collect_once(request, state, body):
    target = state['target'];diagnostics = []
    try:
        if any(e['event'] == 'claude_identity_mismatch' for e in state['control']):
            raise ObservationError('claude_identity_mismatch')
        if request.get('preflight') is None:
            raise ObservationError('claude_preflight_unavailable')
        path = Path(request['native_path'])
        if path.parent != Path(request['project_path']) or path.name != request['thread_id'] + '.jsonl':
            raise ObservationError('rollout_unsafe_path')
        raw, parsed = read_rows(path, request['thread_id'])
        segment = select(raw, parsed, request, body)
        observed = observation(segment, body, request['thread_id'])
        target['segments'] = [segment]
        if observed['matches']:
            target.update(thread_id=request['thread_id'], turn_id=segment['turn_id'], segments=[segment])
        else:
            diagnostics.append(issue('target_input_missing'))
        if any(i['reason'] == 'sensitive' for i in segment['omitted']):
            diagnostics.append(issue('sensitive_required_lines'))
    except ObservationError as error:
        diagnostics.append(error.diagnostic)
    except FileNotFoundError:
        diagnostics.append(issue('rollout_not_found'))
    except OSError:
        diagnostics.append(issue('rollout_unreadable'))
    except (ValueError, KeyError, TypeError, AttributeError):
        diagnostics.append(issue('native_selection_invalid'))
    target['diagnostics'] = diagnostics
    target['issues'] = [v['code'] for v in diagnostics]
