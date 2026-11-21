"""Interpret an allowlisted exec stream as reports, never native observations."""
from .common import digest, no_sensitive

UNKNOWN = 'unknown'


def summarize(request, control):
    result = {'thread_id': UNKNOWN, 'turn_id': UNKNOWN, 'turn_started': UNKNOWN,
              'turn_status': UNKNOWN, 'final_output_sha256': UNKNOWN, 'final_message': None}
    if request['via'] != 'exec':
        return result
    threads = {e['thread_id'] for e in control if e['event'] == 'stream_thread'}
    starts = [(i, e) for i, e in enumerate(control) if e['event'] == 'stream_turn_started']
    terminals = [(i, e) for i, e in enumerate(control) if e['event'] in ('stream_turn_completed', 'stream_turn_failed')]
    ids = {e['turn_id'] for _, e in starts + terminals if e.get('turn_id')}
    if len(threads) != 1 or (request['thread_id'] and request['thread_id'] not in threads):
        return result
    result['thread_id'] = next(iter(threads))
    if (len(starts) != 1 or len(ids) > 1 or
            any(e['event'] == 'stream_identity_mismatch' for e in control)):
        return result
    result['turn_id'] = next(iter(ids)) if ids else UNKNOWN
    result['turn_started'] = True
    if len(terminals) != 1 or terminals[0][0] <= starts[0][0]:
        return result
    result['turn_status'] = 'completed' if terminals[0][1]['event'] == 'stream_turn_completed' else 'failed'
    result['reported_at'] = terminals[0][1].get('at')
    if result['turn_status'] == 'completed':
        candidates = [e for i, e in enumerate(control) if starts[0][0] < i < terminals[0][0]
                      and e['event'] in ('stream_final_message', 'stream_final_message_omitted')]
        if candidates and candidates[-1]['event'] == 'stream_final_message':
            text = candidates[-1]['text']
            no_sensitive(text)
            if digest(text) != candidates[-1]['sha256']:
                raise ValueError('stream final message hash mismatch')
            result.update(final_output_sha256=digest(text), final_message=text)
    return result
