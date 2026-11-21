"""Experimental Antigravity 1.5.9 profile, with selected SQLite wire evidence."""
import base64
import os
import re
import shutil
import sqlite3
import subprocess
from pathlib import Path

from .claude import private_check, session_id
from .common import canonical, digest, now
from .diagnostics import ObservationError, issue, stored_issues
from .protowire import fields, nested, one

PROFILE = 'agy-selected-fields-v1'
VERIFIED = {'version': '1.5.9', 'binary_sha256': '39816d49d5ab92229952987225c23ea3fed7580699dcb4a444e0388e0ac1ce43'}
UNKNOWN = 'unknown'
USER_INPUT, PLANNER_RESPONSE = 14, 15
TERMINATION = {2: 'USER_CANCELED', 4: 'NO_TOOL_CALL'}
STAT_FIELDS = ('st_dev', 'st_ino', 'st_size', 'st_mtime_ns', 'st_ctime_ns')


def extra_arguments(extra):
    result, i = [], 0
    while i < len(extra):
        key, eq, value = extra[i].partition('=')
        if key == '--disable-slash-commands' and not eq:
            result.append(key)
        elif key in ('--mode', '--settings', '--print-timeout'):
            if not eq:
                i += 1
                if i == len(extra):
                    raise ValueError('agy passthrough option requires one value')
                value = extra[i]
            if not value or value.startswith('-'):
                raise ValueError('agy passthrough option requires one value')
            private_check(value)
            if eq:
                result.append(extra[i])
            else:
                result.extend((key, value))
        else:
            raise ValueError('unsupported or wrapper-owned agy passthrough argument')
        i += 1
    return result


def native_path(root, conversation):
    return Path(root) / (session_id(conversation) + '.db')


def row_hash(row):
    return digest(canonical([digest(v) if isinstance(v, bytes) else v for v in row])) if row else digest(b'')


def read_database(path, expected, preflight=None, boundary_only=False):
    """One short read transaction; never copy, checkpoint, or use immutable mode."""
    path = Path(path).absolute()
    if not path.is_file():
        raise ObservationError('rollout_not_found')
    connection = None
    try:
        connection = sqlite3.connect(path.as_uri() + '?mode=ro', uri=True, timeout=0.05)
        connection.execute('BEGIN')
        meta = connection.execute('SELECT cascade_id, trajectory_id FROM trajectory_meta').fetchall()
        if len(meta) != 1 or meta[0][0] != expected:
            raise ObservationError('agy_native_identity_mismatch')
        trajectory = session_id(meta[0][1])
        maximum = connection.execute('SELECT max(idx) FROM steps').fetchone()[0]
        maximum = -1 if maximum is None else maximum
        if type(maximum) is not int or maximum < -1:
            raise ValueError('invalid native step boundary')
        minimum = preflight['max_idx'] if preflight else -1
        boundary_index = maximum if boundary_only else minimum
        boundary = connection.execute('SELECT idx, step_type, status, metadata, step_payload FROM steps WHERE idx=?',
                                      (boundary_index,)).fetchone()
        boundary_sha = row_hash(boundary)
        if preflight and (trajectory != preflight['trajectory_id'] or maximum < minimum or
                          boundary_sha != preflight['boundary_sha256']):
            raise ObservationError('agy_boundary_changed')
        if boundary_only:
            return {'max_idx': maximum, 'boundary_sha256': boundary_sha, 'trajectory_id': trajectory}
        steps = connection.execute('SELECT idx, step_type, status, metadata, step_payload FROM steps WHERE idx>? ORDER BY idx',
                                   (minimum,)).fetchall()
        executors = connection.execute('SELECT idx, data FROM executor_metadata ORDER BY idx').fetchall()
        return {'trajectory_id': trajectory, 'steps': steps, 'executors': executors}
    except sqlite3.OperationalError as error:
        code = getattr(error, 'sqlite_errorcode', None)
        # Python 3.10 lacks sqlite_errorcode. Do not persist SQLite's error text.
        busy = code in (sqlite3.SQLITE_BUSY, sqlite3.SQLITE_LOCKED) if code is not None else any(
            word in str(error).lower() for word in ('busy', 'locked'))
        raise ObservationError('agy_native_busy' if busy else 'agy_native_incomplete') from None
    except sqlite3.DatabaseError:
        raise ObservationError('native_selection_invalid') from None
    finally:
        if connection is not None:
            connection.rollback()
            connection.close()


def resolve_executable(executable, cwd):
    executable = str(executable)
    if '/' in executable:
        return os.path.abspath(Path(cwd) / executable)
    search_path = os.pathsep.join(os.path.abspath(Path(cwd) / entry)
                                  for entry in os.environ.get('PATH', os.defpath).split(os.pathsep))
    resolved = shutil.which(executable, path=search_path)
    if resolved is None:
        raise ValueError('agy executable not found in child PATH')
    return resolved


def request_fields(executable, resume, model, effort, cwd, root, extra):
    extras = extra_arguments(extra)
    sid = session_id(resume) if resume else None
    root = Path(root or Path.home() / '.gemini/antigravity-cli/conversations').absolute()
    # Path construction is not discovery: only the exact returned/requested DB is read.
    path = native_path(root, sid) if sid else None
    preflight = None
    if sid:
        try:
            preflight = read_database(path, sid, boundary_only=True)
        except (OSError, ValueError, TypeError):
            raise ValueError('agy resume requires a readable matching DB and step boundary; no dispatch') from None
    args = [resolve_executable(executable, cwd), '--input-format', 'stream-json', '--output-format', 'stream-json']
    if sid:
        args.extend(('--conversation', sid))
    if model:
        args.extend(('--model', model))
    if effort:
        args.extend(('--effort', effort))
    args.extend(extras)
    private_check(canonical(args))
    return {'thread_id': sid, 'session_root': str(root), 'native_path': str(path) if path else None,
            'preflight': preflight, 'argv': args, 'engine_status': 'experimental'}


def fingerprint(request):
    """Read version and executable hash for dispatch or recovery."""
    executable = request['argv'][0]
    result = {'version': UNKNOWN, 'binary_sha256': UNKNOWN}
    try:
        path = Path(executable)
        stat = path.stat()
        result['stat'] = {name: getattr(stat, name) for name in STAT_FIELDS}
        result['binary_sha256'] = digest(path.read_bytes())
        output = subprocess.run([executable, '--version'], cwd=request['cwd'], stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, timeout=5, check=False)
        value = output.stdout.decode().strip()
        if output.returncode == 0 and re.fullmatch(r'\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?', value):
            result['version'] = value
    except (OSError, UnicodeError, subprocess.TimeoutExpired):
        pass
    return result


def version_verified(control):
    before = [e for e in control if e['event'] == 'agy_version_before']
    after = [e for e in control if e['event'] == 'agy_version_after']
    recovered = [e for e in control if e['event'] == 'agy_version_recover']
    if len(before) != 1 or len(after) > 1:
        return False
    if not after and not recovered:
        return False
    if not all(e.get(k) == v for e in before + after + recovered for k, v in VERIFIED.items()):
        return False
    if after:
        return True
    initial_stat = before[0].get('stat')
    latest_stat = recovered[-1].get('stat')
    return (isinstance(initial_stat, dict) and isinstance(latest_stat, dict) and
            all(type(initial_stat.get(name)) is int and type(latest_stat.get(name)) is int and
                initial_stat[name] == latest_stat[name]
                for name in STAT_FIELDS))


def stream_events(event):
    if not isinstance(event, dict):
        raise ValueError('invalid agy stream event')
    kind = event.get('event')
    if kind not in ('init', 'step_update', 'result'):
        return
    payload = event if kind == 'init' else event.get(kind)
    if not isinstance(payload, dict):
        raise ValueError('invalid agy stream payload')
    sid = session_id(payload.get('conversation_id'))
    yield 'agy_identity', {'conversation_id': sid}
    if kind == 'step_update' and payload.get('step_type') == 'user_input':
        index = payload.get('step_index')
        if type(index) is not int or index < 0:
            yield 'agy_input_index_invalid', {}
            return
        yield 'agy_user_input', {'conversation_id': sid, 'step_index': index}
    if kind == 'result':
        status = payload.get('status')
        reason = payload.get('finished_reason')
        if reason is not None and reason not in ('interrupted', 'print_timeout'):
            reason = UNKNOWN
        value = {'conversation_id': sid, 'status': status if status in ('SUCCESS', 'ERROR') else UNKNOWN,
                 'finished_reason': reason}
        text = payload.get('response')
        if status == 'SUCCESS' and not isinstance(text, str):
            value['malformed'] = True
        elif status == 'SUCCESS':
            try:
                text.encode('utf-8')
            except UnicodeError:
                value['malformed'] = True
            else:
                if reason is None:
                    try:
                        private_check(text)
                    except ValueError:
                        value['output_omitted'] = 'sensitive'
                    else:
                        value.update(text=text, sha256=digest(text))
        yield 'agy_result', value


def stream_identity(request, control):
    ids = {e['conversation_id'] for e in control if e['event'] == 'agy_identity'}
    # Validate even offline; identity must come from this child's preserved stdout.
    for value in ids:
        session_id(value)
    mismatch = len(ids) > 1 or bool(ids and request['thread_id'] and request['thread_id'] not in ids)
    return (next(iter(ids)) if len(ids) == 1 else None), mismatch


def input_index(control, conversation):
    events = [e for e in control if e['event'] == 'agy_user_input']
    indexes = {e['step_index'] for e in events if e.get('conversation_id') == conversation}
    if (any(e.get('conversation_id') != conversation for e in events) or len(indexes) != 1 or
            any(type(v) is not int or v < 0 for v in indexes) or
            any(e['event'] in ('agy_input_index_invalid', 'agy_stream_incomplete') for e in control)):
        return None
    return next(iter(indexes))


def reported(request, control):
    result = {'thread_id': UNKNOWN, 'turn_id': UNKNOWN, 'turn_started': UNKNOWN,
              'turn_status': UNKNOWN, 'final_output_sha256': UNKNOWN, 'final_message': None}
    sid, mismatch = stream_identity(request, control)
    if mismatch:
        return result
    result['thread_id'] = sid or UNKNOWN
    terminals = [e for e in control if e['event'] == 'agy_result']
    if len(terminals) != 1 or any(e['event'] == 'agy_stream_incomplete' for e in control):
        return result
    event = terminals[0]
    if not sid or event.get('conversation_id') != sid or event.get('malformed'):
        return result
    status, reason = event.get('status'), event.get('finished_reason')
    if status == 'SUCCESS':
        if reason is None:
            result['turn_status'] = 'completed'
        elif reason == 'print_timeout':
            result['turn_status'] = 'failed'
    elif status == 'ERROR':
        result['turn_status'] = 'interrupted' if reason == 'interrupted' else 'failed'
    result['reported_at'] = event.get('at')
    if result['turn_status'] == 'completed' and isinstance(event.get('text'), str):
        private_check(event['text'])
        if digest(event['text']) != event.get('sha256'):
            raise ValueError('agy stream output hash mismatch')
        result.update(final_message=event['text'], final_output_sha256=event['sha256'])
    return result


def text(field):
    value = field.value.decode('utf-8')
    private_check(value)
    return value


def fragment(cell, field, table, pk, column, path):
    # Only the leaf field's tag/length/value bytes; never its enclosing message.
    return {'kind': 'original_wire_field', 'profile': PROFILE, 'table': table, 'pk': pk,
            'column': column, 'field_path': path, 'cell_start': field.start, 'cell_end': field.end,
            'cell_length': len(cell), 'cell_sha256': digest(cell),
            'wire_base64': base64.b64encode(cell[field.start:field.end]).decode('ascii'),
            'sha256': digest(cell[field.start:field.end])}


def step_projection(row):
    index, sql_type, sql_status, metadata, cell = row
    fs = fields(cell)
    typ, status = one(fs, 1, 0), one(fs, 4, 0)
    if typ.value != sql_type or status.value != sql_status:
        raise ValueError('agy SQL/protobuf step mismatch')
    meta = one(fs, 5, 2)
    execution = one(nested(meta), 12, 2)
    execution_id = session_id(text(execution))
    selected = [([1], typ), ([4], status), ([5, 12], execution)]
    extra = []
    if metadata is not None:
        # SQLite duplicates Step.metadata in a separate cell in this version.
        mf = one(fields(metadata), 12, 2)
        if text(mf) != execution_id:
            raise ValueError('agy SQL/protobuf metadata mismatch')
        extra.append(fragment(metadata, mf, 'steps', index, 'metadata', [12]))
    projection = {'kind': 'derived_projection', 'idx': index, 'step_type': sql_type,
                  'status': sql_status, 'execution_id': execution_id}
    if sql_type == USER_INPUT:
        user = one(fs, 19, 2)
        response = one(nested(user), 2, 2)
        projection['input_sha256'] = digest(text(response))
        selected.append(([19, 2], response))
    elif sql_type == PLANNER_RESPONSE:
        planner = one(fs, 20, 2)
        pf = nested(planner)
        for number, name in ((1, 'response_sha256'), (8, 'modified_response_sha256')):
            response = one(pf, number, 2, required=False)
            if response is not None:
                projection[name] = digest(text(response))
                selected.append(([20, number], response))
    return projection, [fragment(cell, f, 'steps', index, 'step_payload', path) for path, f in selected] + extra


def executor_projection(row):
    index, cell = row
    fs = fields(cell)
    reason, execution = one(fs, 1, 0), one(fs, 9, 2)
    projection = {'kind': 'derived_projection', 'idx': index, 'execution_id': session_id(text(execution)),
                  'termination_reason': reason.value}
    return projection, [fragment(cell, f, 'executor_metadata', index, 'data', [n]) for n, f in ((1, reason), (9, execution))]


def select(snapshot, request, conversation, index, body):
    rows = snapshot['steps']
    minimum = (request.get('preflight') or {}).get('max_idx', -1)
    users = [r for r in rows if r[0] == index and r[1] == USER_INPUT]
    if len(users) != 1 or index <= minimum:
        raise ObservationError('ambiguous_target_match' if len(users) > 1 else 'no_matching_turn')
    user, fragments = step_projection(users[0])
    if user['input_sha256'] != digest(body):
        raise ObservationError('target_input_missing')
    steps = [user]
    omitted = []
    for row in rows:
        if row[0] == index:
            continue
        # Only planner responses can supply an auxiliary output hash; tools stay omitted.
        if row[1] != PLANNER_RESPONSE:
            continue
        try:
            projection, parts = step_projection(row)
            if projection['execution_id'] == user['execution_id']:
                steps.append(projection)
                fragments.extend(parts)
        except (ValueError, TypeError):
            omitted.append({'kind': 'omitted_projection', 'table': 'steps', 'pk': row[0], 'reason': 'invalid_or_sensitive'})
    executors = []
    for row in snapshot['executors']:
        try:
            projection, parts = executor_projection(row)
            if projection['execution_id'] == user['execution_id']:
                executors.append(projection)
                fragments.extend(parts)
        except (ValueError, TypeError):
            omitted.append({'kind': 'omitted_projection', 'table': 'executor_metadata', 'pk': row[0], 'reason': 'invalid_or_sensitive'})
    return {'format': PROFILE, 'conversation_id': conversation, 'trajectory_id': snapshot['trajectory_id'],
            'input_index': index, 'turn_id': user['execution_id'], 'steps': steps, 'executors': executors,
            'fragments': fragments, 'omitted': omitted, 'observed_at': now()}


def validate_fragment(part):
    if part['kind'] != 'original_wire_field' or part['profile'] != PROFILE:
        raise ValueError('invalid agy wire fragment profile')
    raw = base64.b64decode(part['wire_base64'], validate=True)
    if (len(raw) != part['cell_end'] - part['cell_start'] or not 0 <= part['cell_start'] < part['cell_end'] <= part['cell_length']
            or digest(raw) != part['sha256'] or not re.fullmatch('[0-9a-f]{64}', part['cell_sha256'])):
        raise ValueError('agy wire fragment commitment mismatch')
    fs = fields(raw)
    if len(fs) != 1 or fs[0].number != part['field_path'][-1]:
        raise ValueError('invalid agy leaf wire field')
    return fs[0]


def observation(segment, body, request, control):
    """Recompute from selected leaf bytes, checking every derived projection."""
    if segment.get('format') != PROFILE:
        raise ValueError('invalid agy selected-field profile')
    sid, mismatch = stream_identity(request, control)
    index = input_index(control, sid)
    minimum = (request.get('preflight') or {}).get('max_idx', -1)
    if (mismatch or sid != segment['conversation_id'] or index != segment['input_index'] or index is None or index <= minimum
            or (request.get('preflight') and request['preflight']['trajectory_id'] != segment['trajectory_id'])):
        raise ValueError('agy preserved stream/native binding mismatch')
    session_id(segment['trajectory_id'])
    mapped, cells = {}, {}
    allowed = {('steps', 'step_payload'): {(1,), (4,), (5, 12), (19, 2), (20, 1), (20, 8)},
               ('steps', 'metadata'): {(12,)}, ('executor_metadata', 'data'): {(1,), (9,)}}
    for part in segment['fragments']:
        pair = part['table'], part['column']
        path = tuple(part['field_path'])
        if path not in allowed.get(pair, set()):
            raise ValueError('unsupported agy selected field')
        key = (*pair, part['pk'], path)
        if key in mapped:
            raise ValueError('duplicate agy selected field')
        field = validate_fragment(part)
        numeric = (pair == ('steps', 'step_payload') and path in ((1,), (4,)) or
                   pair[0] == 'executor_metadata' and path == (1,))
        expected_wire = 0 if numeric else 2
        if field.wire != expected_wire:
            raise ValueError('mistyped agy selected field')
        mapped[key] = field.value if field.wire == 0 else text(field)
        cell_key = (*pair, part['pk'])
        spans = cells.setdefault(cell_key, [])
        for old in spans:
            if (old['cell_sha256'] != part['cell_sha256'] or old['cell_length'] != part['cell_length'] or
                    max(old['cell_start'], part['cell_start']) < min(old['cell_end'], part['cell_end'])):
                raise ValueError('inconsistent agy cell fragment provenance')
        spans.append(part)
    used = set()

    def leaf(table, column, pk, path, optional=False):
        key = table, column, pk, path
        if key not in mapped and optional:
            return None
        used.add(key)
        return mapped[key]

    steps = []
    for row in segment['steps']:
        pk = row['idx']
        typ = leaf('steps', 'step_payload', pk, (1,))
        derived = {'kind': 'derived_projection', 'idx': pk, 'step_type': typ,
                   'status': leaf('steps', 'step_payload', pk, (4,)),
                   'execution_id': session_id(leaf('steps', 'step_payload', pk, (5, 12)))}
        sql_execution = leaf('steps', 'metadata', pk, (12,), optional=True)
        if sql_execution is not None and sql_execution != derived['execution_id']:
            raise ValueError('agy selected metadata conflict')
        if typ == USER_INPUT:
            derived['input_sha256'] = digest(leaf('steps', 'step_payload', pk, (19, 2)))
        elif typ == PLANNER_RESPONSE:
            for n, name in ((1, 'response_sha256'), (8, 'modified_response_sha256')):
                value = leaf('steps', 'step_payload', pk, (20, n), optional=True)
                if value is not None:
                    derived[name] = digest(value)
        else:
            raise ValueError('unsupported agy selected step')
        if row != derived:
            raise ValueError('agy wire/projection mismatch')
        steps.append(derived)
    users = [r for r in steps if r['step_type'] == USER_INPUT]
    if (len(users) != 1 or users[0]['idx'] != index or users[0]['input_sha256'] != digest(body) or
            users[0]['execution_id'] != segment['turn_id'] or len({r['idx'] for r in steps}) != len(steps) or
            any(r['idx'] < index or r['execution_id'] != segment['turn_id'] for r in steps)):
        raise ValueError('agy selected input/execution mismatch')
    reasons, executor_ids = [], set()
    for row in segment['executors']:
        pk = row['idx']
        derived = {'kind': 'derived_projection', 'idx': pk,
                   'execution_id': session_id(leaf('executor_metadata', 'data', pk, (9,))),
                   'termination_reason': leaf('executor_metadata', 'data', pk, (1,))}
        if row != derived or row['execution_id'] != segment['turn_id'] or pk in executor_ids:
            raise ValueError('agy selected executor mismatch')
        executor_ids.add(pk)
        reasons.append(row['termination_reason'])
    if used != set(mapped):
        raise ValueError('unreferenced agy selected bytes')
    planners = sorted((r for r in steps if r['step_type'] == PLANNER_RESPONSE), key=lambda r: r['idx'])
    output_hash = planners[-1].get('response_sha256', UNKNOWN) if planners else UNKNOWN
    # An omitted later response cannot be replaced with an earlier response.
    if any(r['table'] == 'steps' and (not planners or r['pk'] >= planners[-1]['idx']) for r in segment['omitted']):
        output_hash = UNKNOWN
    return {'matches': True, 'execution_id': segment['turn_id'], 'trajectory_id': segment['trajectory_id'],
            'native_output_sha256': output_hash, 'termination_reasons': reasons}


def collect_once(request, state, body):
    target = state['target']
    diagnostics = []
    try:
        sid, mismatch = stream_identity(request, state['control'])
        if mismatch:
            raise ObservationError('agy_identity_mismatch')
        if not sid:
            raise ObservationError('agy_identity_unavailable')
        index = input_index(state['control'], sid)
        if index is None:
            raise ObservationError('agy_input_index_unavailable')
        snapshot = read_database(native_path(request['session_root'], sid), sid, request.get('preflight'))
        segment = select(snapshot, request, sid, index, body)
        observation(segment, body, request, state['control'])
        target.update(thread_id=sid, segments=[segment])
        if segment['omitted']:
            diagnostics.append(issue('agy_fields_omitted'))
        if not segment['executors']:
            diagnostics.append(issue('agy_executor_pending'))
    except ObservationError as error:
        diagnostics.append(error.diagnostic)
        if error.diagnostic['code'] in ('agy_input_index_unavailable', 'agy_identity_mismatch', 'agy_identity_unavailable'):
            target['segments'] = []
    except (OSError, ValueError, TypeError, KeyError):
        diagnostics.append(issue('native_selection_invalid'))
    target['diagnostics'] = diagnostics
    target['issues'] = [d['code'] for d in diagnostics]


def inspect(source, request, state, body, facts, rejection):
    control, target = state['control'], state['target']
    verified = version_verified(control)
    sid, mismatch = stream_identity(request, control)
    report = reported(request, control)
    auxiliary = {'native_output_sha256': UNKNOWN, 'matches_reported_output': UNKNOWN, 'termination_reasons': UNKNOWN}
    unverified = []
    execution, trajectory = UNKNOWN, UNKNOWN
    reasons = []
    segments = target.get('segments', [])
    if len(segments) > 1:
        raise ValueError('multiple agy native selections')
    for segment in segments:
        native = observation(segment, body, request, control)
        if verified:
            facts['target_input_confirmed'] = True
            execution, trajectory = native['execution_id'], native['trajectory_id']
            reasons = native['termination_reasons']
            auxiliary.update(native_output_sha256=native['native_output_sha256'],
                             termination_reasons=[{'value': n, **({'name': TERMINATION[n]} if n in TERMINATION else {})}
                                                  for n in native['termination_reasons']])
        else:
            unverified.append({'assessment': 'unverified_interpretation', 'input_bytes_match': native['matches'],
                               'termination_reason_values': native['termination_reasons']})
    observed = dict(facts)
    levels = {k: 'observed' if v != UNKNOWN else UNKNOWN for k, v in facts.items()}
    conflict = verified and report['turn_status'] == 'completed' and 2 in reasons
    if not conflict and not mismatch:
        for key in ('turn_status', 'final_output_sha256'):
            if report[key] != UNKNOWN:
                facts[key], levels[key] = report[key], 'reported'
    if auxiliary['native_output_sha256'] != UNKNOWN and report['final_output_sha256'] != UNKNOWN:
        auxiliary['matches_reported_output'] = auxiliary['native_output_sha256'] == report['final_output_sha256']
    diagnostics = list(stored_issues(target))
    for condition, code in ((not verified, 'agy_version_unverified'), (mismatch, 'agy_identity_mismatch'),
                            (conflict, 'agy_status_conflict'), (report['turn_status'] == UNKNOWN, 'agy_result_unavailable')):
        if condition and code not in [d['code'] for d in diagnostics]:
            diagnostics.append(issue(code))
    outputs = [e for e in control if e['event'] in ('output_written', 'output_unavailable')]
    output = None
    if outputs:
        last = outputs[-1]
        output = ({'status': 'written', 'sha256': last['sha256'], 'assessment': last['assessment'],
                   'source': last['source'], 'path': last['path']} if last['event'] == 'output_written'
                  else {'status': 'not_written', 'reason': last['reason']})
    conversation = sid or request['thread_id'] or UNKNOWN
    return {'engine': 'agy', 'engine_status': 'experimental', 'interpretation_profile': PROFILE,
            'version_verified': verified, 'verified': dict(VERIFIED),
            'version_observations': [e for e in control if e['event'] in ('agy_version_before', 'agy_version_after', 'agy_version_recover')],
            'attempt_id': source['attempt_id'], 'name': source['name'], 'group': source['group'], 'retry_of': source['retry_of'],
            'instruction_sha256': source['sha256'], 'source_path': source['path'], 'created_at': source['captured_at'],
            'via': request['via'], 'thread_id': conversation, 'conversation_id': conversation,
            'requested_conversation_id': request['thread_id'] or UNKNOWN,
            'actual_conversation_ids': sorted({e['conversation_id'] for e in control if e['event'] == 'agy_identity'}),
            'trajectory_id': trajectory, 'turn_id': execution, 'turn_key': {'kind': 'execution', 'value': execution},
            'facts': facts, 'fact_levels': levels, 'observed_facts': observed,
            'reported': {k: v for k, v in report.items() if k != 'final_message'},
            'identity_levels': {'thread_id': 'observed' if facts['target_input_confirmed'] is True else 'reported' if sid else 'requested' if request['thread_id'] else UNKNOWN,
                                'turn_id': 'observed' if execution != UNKNOWN else UNKNOWN},
            'auxiliary': auxiliary, 'unverified_interpretation': unverified,
            'output': output, 'rejection_reason': rejection, 'issues': [d['code'] for d in diagnostics],
            'observation_issues': diagnostics, 'observation_attempts': target.get('observation_attempts', []),
            'assessment': levels['turn_status']}
