"""Deterministic facts from preserved E only; no native filesystem access."""
from pathlib import Path
import re
from .common import TERMINAL, dispatch_facts, parse_timestamp
from .rollout import observation
from .reported import summarize as stream_summary
from .diagnostics import stored_issues

UNKNOWN = 'unknown'


def inspect(store, attempt):
    source, request, state, body = store.load(attempt)
    control, target = state['control'], state['target']
    engine = source.get('engine', 'codex')
    facts = {**dispatch_facts(request['via'], control), 'process_started': UNKNOWN, 'process_exit_code': UNKNOWN,
             'target_input_confirmed': UNKNOWN, 'turn_started': UNKNOWN,
             'turn_status': UNKNOWN, 'completed_at': UNKNOWN, 'final_output_sha256': UNKNOWN,
             'model_read_or_used': UNKNOWN}
    for event in control:
        if event['event'] == 'process_started':
            facts['process_started'] = True
        if event['event'] == 'process_exited':
            facts['process_exit_code'] = event['exit_code']
    rejection = next((e['reason'] for e in reversed(control) if e['event'] == 'dispatch_rejected'), None)
    if facts['dispatched'] is False and rejection is None and any(e['event'] == 'daemon_error' for e in control):
        rejection = 'daemon preflight failed; specific reason unavailable in this legacy bundle'
    if facts['dispatched'] is False and rejection is None and any(e['event'] == 'process_error' for e in control):
        rejection = 'native process could not be started'
    if engine == 'agy':
        from .agy import inspect as inspect_agy
        return inspect_agy(source, request, state, body, facts, rejection)
    auxiliary = None
    native_traces = []
    if engine == 'claude':
        from . import claude
        retained = []
        if target.get('thread_id') not in (None, request['thread_id']):
            raise ValueError('Claude stored session binding mismatch')
        auxiliary = {'native_message_end': UNKNOWN, 'native_output_sha256': UNKNOWN, 'native_message_timestamp': UNKNOWN}
        if not any(e['event'] == 'claude_identity_mismatch' for e in control):
            for segment in target['segments']:
                native = claude.observation(segment, body, request['thread_id'])
                retained.append(native)
                if native['matches']:
                    if target.get('turn_id') != segment['turn_id']:
                        raise ValueError('Claude stored user-message binding mismatch')
                    facts['target_input_confirmed'] = True
                native_traces.extend(native['native_traces'])
                auxiliary.update(native['auxiliary'])
    else:
        retained = []
        for segment in target['segments']:
            if segment['turn_id'] != target.get('turn_id'):
                raise ValueError('observation turn binding mismatch')
            native, _ = observation(segment, segment['copy_text'].encode(), body)
            retained.append(native)
            if native['matches']:
                facts['target_input_confirmed'] = True
            if native['started']:
                facts['turn_started'] = True
            if native['terminal']:
                facts['turn_status'] = native['terminal']
                facts['completed_at'] = native['completed_at'] or UNKNOWN
            if native['final_output_sha256']:
                facts['final_output_sha256'] = native['final_output_sha256']
        exact = target.get('exact_status')
        if exact and exact['id'] == target.get('turn_id') and exact['status'] in TERMINAL:
            normalized = {'cancelled': 'interrupted', 'aborted': 'interrupted'}.get(exact['status'], exact['status'])
            if facts['turn_status'] not in (UNKNOWN, normalized):
                raise ValueError('conflicting terminal observations')
            facts['turn_status'] = normalized
    observed = dict(facts)
    levels = {name: 'observed' if value != UNKNOWN else UNKNOWN for name, value in facts.items()}
    reported = claude.reported(request, control) if engine == 'claude' else stream_summary(request, control)
    conflict = engine == 'claude' and reported['turn_status'] == 'completed' and any(
        t['kind'] in ('synthetic_error', 'interruption') for t in native_traces)
    for name in ('turn_started', 'turn_status', 'final_output_sha256'):
        if conflict and name in ('turn_status', 'final_output_sha256'):
            continue
        if engine == 'claude' and name == 'final_output_sha256' and facts['turn_status'] != 'completed':
            continue
        if facts[name] == UNKNOWN and reported[name] != UNKNOWN:
            facts[name], levels[name] = reported[name], 'reported'
    outputs = [e for e in control if e['event'] in ('output_written', 'output_unavailable')]
    output = None
    if outputs:
        last = outputs[-1]
        output = ({'status': 'written', 'sha256': last['sha256'],
                   'assessment': last.get('assessment', 'observed'),
                   'source': last.get('source', 'target_rollout'), 'path': last.get('path', request['output'])}
                  if last['event'] == 'output_written' else {'status': 'not_written', 'reason': last['reason']})
    thread = target.get('thread_id') or request['thread_id'] or UNKNOWN
    turn = target.get('turn_id') or reported['turn_id'] or UNKNOWN
    result = {'engine': engine, 'interpretation_profile': source.get('interpretation_profile', 'codex-selected-lines-v2'),
            'attempt_id': attempt, 'name': source['name'], 'group': source['group'],
            'retry_of': source['retry_of'], 'instruction_sha256': source['sha256'],
            'source_path': source['path'], 'created_at': source['captured_at'],
            'via': request['via'], 'thread_id': thread, 'turn_id': turn, 'facts': facts,
            'fact_levels': levels, 'observed_facts': observed,
            'reported': {k: v for k, v in reported.items() if k != 'final_message'},
            'identity_levels': {'thread_id': 'observed' if retained else 'reported' if reported['thread_id'] != UNKNOWN else 'requested' if request['thread_id'] else UNKNOWN,
                                'turn_id': 'observed' if retained else 'reported' if reported['turn_id'] != UNKNOWN else UNKNOWN},
            'output': output, 'rejection_reason': rejection, 'issues': target.get('issues', []),
            'observation_issues': stored_issues(target),
            'observation_attempts': target.get('observation_attempts', []),
            'assessment': levels['turn_status']}
    if engine == 'claude':
        confirmed = facts['target_input_confirmed'] is True
        result['turn_id'] = target.get('turn_id', UNKNOWN) if confirmed else UNKNOWN
        result['turn_key'] = {'kind': 'user_message', 'value': result['turn_id']}
        result['identity_levels']['turn_id'] = 'observed' if confirmed else UNKNOWN
        result['identity_levels']['thread_id'] = 'observed' if confirmed else 'reported' if reported['thread_id'] != UNKNOWN else 'requested'
        auxiliary['matches_reported_output'] = (auxiliary['native_output_sha256'] == reported['final_output_sha256']
            if auxiliary['native_output_sha256'] != UNKNOWN and reported['final_output_sha256'] != UNKNOWN else UNKNOWN)
        result['auxiliary'] = auxiliary
        result['native_traces'] = native_traces
        if conflict:
            from .diagnostics import issue
            diagnostic = issue('claude_status_conflict')
            result['observation_issues'] = [*result['observation_issues'], diagnostic]
            result['issues'] = [*result['issues'], diagnostic['code']]
        if reported['turn_status'] == UNKNOWN:
            from .diagnostics import issue
            diagnostic = issue('claude_result_unavailable')
            result['observation_issues'] = [*result['observation_issues'], diagnostic]
            result['issues'] = [*result['issues'], diagnostic['code']]
    return result



def records(store, engine=None):
    store.warnings.clear()
    result = []
    for aid in store.ids():
        try:
            row = inspect(store, aid)
            if engine is None or row['engine'] == engine:
                result.append(row)
        except (OSError, ValueError, KeyError, TypeError, IndexError, AttributeError) as error:
            store.warn_skipped(aid, error)
    return result


def lookup(store, selector, engine=None):
    if ':' in selector:
        qualifier, selector = selector.split(':', 1)
        if qualifier not in ('codex', 'claude', 'agy') or engine not in (None, qualifier):
            raise ValueError('invalid or conflicting engine qualifier')
        engine = qualifier
    matches = [r for r in records(store, engine) if selector in (r['attempt_id'], r['turn_id']) and selector != UNKNOWN]
    if len({r['engine'] for r in matches}) > 1:
        raise ValueError('native ID matches multiple engines; specify --engine or ENGINE:ID')
    if not matches and not any(w['attempt_id'] == selector for w in store.warnings):
        raise ValueError('attempt/turn not found in this store')
    return matches


def duplicates(store, selector, engine=None):
    if selector.startswith('agy:'):
        if engine not in (None, 'agy'):
            raise ValueError('conflicting engine qualifier')
        engine, selector = 'agy', selector[4:]
    all_rows = records(store, engine)
    selected = [r for r in all_rows if selector in (r['attempt_id'], r['group']) or
                (r['engine'] == 'agy' and selector == r['turn_id'] and selector != UNKNOWN)]
    if not selected:
        if any(w['attempt_id'] == selector for w in store.warnings):
            return {'group': selector, 'attempts': [], 'duplicate_execution': UNKNOWN,
                    'effective_attempt': UNKNOWN, 'incomplete': True}
        raise ValueError('attempt/group not found')
    group = selected[0]['group']
    rows = [r for r in all_rows if r['group'] == group]
    confirmed = [r for r in rows if r['facts']['target_input_confirmed'] is True]
    # Claude/agy whole-turn terminals are reported. Confirmed input pairs alone
    # do not establish resolved duplicate execution in these profiles.
    reported_group = all(r['engine'] in ('claude', 'agy') for r in rows)
    terminal_key = 'facts' if reported_group else 'observed_facts'
    terminals = not store.warnings and all(r['facts']['dispatched'] is False or
                   (r[terminal_key]['turn_status'] in TERMINAL and r in confirmed) for r in rows)
    counted = [r for r in confirmed if r['facts']['turn_status'] in TERMINAL] if reported_group else confirmed
    turns = {(r['engine'], r['thread_id'], r.get('trajectory_id'), r['turn_id']) for r in counted}
    effective = UNKNOWN
    completed = [r for r in rows if r[terminal_key]['turn_status'] == 'completed']
    if terminals and not completed:
        effective = None
    elif terminals and all(r['facts']['completed_at'] != UNKNOWN for r in completed):
        ordered = sorted(completed, key=lambda r: parse_timestamp(r['facts']['completed_at']))
        if len(ordered) == 1 or parse_timestamp(ordered[0]['facts']['completed_at']) < parse_timestamp(ordered[1]['facts']['completed_at']):
            effective = ordered[0]['attempt_id']
    return {'group': group, 'attempts': [r['attempt_id'] for r in rows],
            'duplicate_execution': True if len(turns) > 1 else False if terminals else UNKNOWN,
            'effective_attempt': effective, 'incomplete': bool(store.warnings), 'scope': 'explicit retry group in this store',
            'meaning': 'earliest observed completion; no claim about semantic effect'}


def which(store, selector, engine=None):
    if selector.startswith('agy:'):
        if engine not in (None, 'agy'):
            raise ValueError('conflicting engine qualifier')
        engine, selector = 'agy', selector[4:]
    # A path is an identifier; never open or stat the original instruction.
    key = selector if re.fullmatch('[0-9a-f]{64}', selector) else str(Path(selector).absolute())
    matches = [r for r in records(store, engine) if key in (r['instruction_sha256'], r['source_path'])]
    return {'query': selector, 'matches': [r for r in matches if r['facts']['target_input_confirmed'] is True],
            'unconfirmed_attempts': [r['attempt_id'] for r in matches if r['facts']['target_input_confirmed'] != True],
            'outside_observation': UNKNOWN, 'model_read_or_used': UNKNOWN}
