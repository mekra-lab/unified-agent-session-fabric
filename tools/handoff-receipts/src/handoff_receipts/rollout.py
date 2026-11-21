"""Pure parsing of preserved target segments; no filesystem or RPC access."""
from .common import digest

def terminal_status(payload):
    kind = payload.get("type")
    if kind == "task_complete":
        return "failed" if payload.get("error") is not None else "completed"
    return {"turn_aborted": "interrupted", "task_failed": "failed"}.get(kind)

def segments(content, parsed, body, minimum=0, include_turn=None):
    """Exact user-message text, within explicit task_started/terminal boundaries."""
    found, active = [], None
    for start, end, row in parsed:
        payload = row.get('payload') or {}
        kind = payload.get('type')
        if row.get('type') == 'event_msg' and kind == 'task_started':
            active = {'turn_id': payload.get('turn_id'), 'start': start, 'end': end,
                      'matches': False, 'completed_at': None, 'terminal': None,
                      'final_output_sha256': None}
        # Compaction changes model context, not the turn lifecycle. Keep the
        # exact start/input binding until another start or a matching terminal.
        if (active is None and include_turn and row.get('type') == 'event_msg'
                and kind in ('task_complete', 'turn_aborted', 'task_failed')
                and payload.get('turn_id') == include_turn):
            active = {'turn_id': include_turn, 'start': start, 'end': end, 'matches': False,
                      'completed_at': None, 'terminal': None, 'final_output_sha256': None}
        if active:
            active['end'] = end
            if row.get('type') == 'response_item' and kind == 'message':
                text = ''.join(p.get('text', '') for p in payload.get('content', [])
                               if p.get('type') in ('text', 'input_text', 'output_text'))
                if payload.get('role') == 'user' and text.encode() == body:
                    active['matches'] = True
                if payload.get('role') == 'assistant' and payload.get('phase') == 'final_answer':
                    active['final_output_sha256'] = digest(text)
            if row.get('type') == 'event_msg' and kind in ('task_complete', 'turn_aborted', 'task_failed'):
                if payload.get('turn_id') not in (None, active['turn_id']):
                    active = None
                    continue
                active['terminal'] = terminal_status(payload)
                active['completed_at'] = row.get('timestamp') if active['terminal'] == 'completed' else None
                if kind == 'task_complete' and isinstance(payload.get('last_agent_message'), str):
                    active['final_output_sha256'] = digest(payload['last_agent_message'])
                if (active['matches'] or active['turn_id'] == include_turn) and active['turn_id'] and active['start'] >= minimum:
                    found.append(active)
                active = None
    if active and (active['matches'] or active['turn_id'] == include_turn) and active['turn_id'] and active['start'] >= minimum:
        found.append(active)
    return found


FORMAT = 'selected-lines-v2'


def line_roles(row, body):
    """Role labels are interpretation metadata, never evidence by themselves."""
    p = row.get('payload') or {}
    kind = p.get('type')
    roles = []
    if row.get('type') == 'event_msg':
        if kind == 'task_started':
            roles.append('start')
        if kind in ('task_complete', 'turn_aborted', 'task_failed'):
            roles.append('terminal')
            if kind == 'task_complete' and isinstance(p.get('last_agent_message'), str):
                roles.append('final_output')
    if row.get('type') == 'response_item' and kind == 'message':
        text = ''.join(v.get('text', '') for v in p.get('content', [])
                       if v.get('type') in ('text', 'input_text', 'output_text'))
        if p.get('role') == 'user' and text.encode() == body:
            roles.append('input')
        if p.get('role') == 'assistant' and p.get('phase') == 'final_answer':
            roles.append('final_output')
    return roles


def selected_facts(segment, parsed, body):
    """Use only retained rows; omitted rows cannot supply a value or a hash fact."""
    turn = segment['turn_id']
    start = next(((a, row) for a, _, row in parsed if
                  row.get('type') == 'event_msg' and row.get('payload', {}).get('type') == 'task_started'
                  and row['payload'].get('turn_id') == turn), None)
    inputs = [(a, row) for a, _, row in parsed if 'input' in line_roles(row, body)
              and start and a > start[0]]
    terminals = [(a, row) for a, _, row in parsed if 'terminal' in line_roles(row, body)
                 and (row['payload'].get('turn_id') == turn or
                      (row['payload'].get('turn_id') is None and start and a > start[0]))]
    terminal = terminals[-1] if terminals else None
    outputs = [(a, row) for a, _, row in parsed if 'final_output' in line_roles(row, body)
               and ((start and a > start[0]) or (terminal and a == terminal[0]))]
    # An omitted later output candidate must not expose a hash computed from its
    # private bytes or allow an earlier assistant message to be called final.
    omitted_outputs = [r['start'] for r in segment.get('omitted', []) if 'final_output' in r.get('roles', [])]
    output_hash = None
    if outputs and (not omitted_outputs or max(omitted_outputs) < outputs[-1][0]):
        p = outputs[-1][1]['payload']
        text = p.get('last_agent_message') if p.get('type') == 'task_complete' else ''.join(
            v.get('text', '') for v in p.get('content', []) if v.get('type') in ('text', 'input_text', 'output_text'))
        output_hash = digest(text)
    return {'matches': bool(inputs), 'started': bool(start),
            'terminal': terminal_status(terminal[1]['payload']) if terminal else None,
            'completed_at': terminal[1].get('timestamp') if terminal and terminal_status(terminal[1]['payload']) == 'completed' else None,
            'final_output_sha256': output_hash}


def select_segment(content, parsed, segment, body):
    """Build a selective copy plus a complete per-line inventory of its span."""
    from .common import canonical, no_sensitive
    selected, omitted, parts, retained = [], [], [], []
    offset = 0
    for a, b, row in parsed:
        if a < segment['start'] or b > segment['end']:
            continue
        raw = content[a:b]
        roles = line_roles(row, body)
        item = {'start': a, 'end': b, 'sha256': digest(raw), 'roles': roles}
        try:
            no_sensitive(raw.decode('utf-8'))
            no_sensitive(canonical(row))
            sensitive = False
        except ValueError:
            sensitive = True
        if not roles or sensitive:
            omitted.append({**item, 'reason': 'sensitive' if sensitive else 'not_required'})
        else:
            selected.append({**item, 'copy_start': offset, 'copy_end': offset + len(raw)})
            parts.append(raw)
            retained.append((a, b, row))
            offset += len(raw)
    copied = b''.join(parts)
    result = {'format': FORMAT, 'turn_id': segment['turn_id'], 'start': segment['start'], 'end': segment['end'],
              'snapshot_sha256': digest(content), 'range_sha256': digest(content[segment['start']:segment['end']]),
              'copy_sha256': digest(copied), 'lines': selected, 'omitted': omitted}
    result.update(selected_facts(result, retained, body))
    return result, copied


def observation(segment, raw, body):
    """Validate selective/legacy E and return facts plus retained original ranges.

    Whole-range and snapshot hashes are capture commitments, not reconstructible
    from selective bytes. Each retained line and the inventory coverage ARE checked.
    """
    import json
    from .common import canonical, no_sensitive
    if segment.get('format') is None:
        if digest(raw) != segment['range_sha256'] or len(raw) != segment['end'] - segment['start']:
            raise ValueError('legacy observation range/hash mismatch')
        parsed, offset = [], segment['start']
        for line in raw.splitlines(keepends=True):
            parsed.append((offset, offset + len(line), json.loads(line)))
            offset += len(line)
        return selected_facts(segment, parsed, body), parsed
    if segment['format'] != FORMAT or digest(raw) != segment['copy_sha256']:
        raise ValueError('unsupported observation format or copy hash mismatch')
    inventory = sorted(segment['lines'] + segment['omitted'], key=lambda r: r['start'])
    cursor = segment['start']
    for item in inventory:
        if item['start'] != cursor or item['end'] <= cursor:
            raise ValueError('observation inventory gap/overlap')
        cursor = item['end']
    if cursor != segment['end']:
        raise ValueError('observation inventory does not cover span')
    parsed, offset = [], 0
    for item in segment['lines']:
        if item['copy_start'] != offset or item['copy_end'] - offset != item['end'] - item['start']:
            raise ValueError('selected line offset mismatch')
        line = raw[offset:item['copy_end']]
        if digest(line) != item['sha256'] or not line.endswith(b'\n') or len(line.splitlines()) != 1:
            raise ValueError('selected line hash/length mismatch')
        row = json.loads(line)
        no_sensitive(line.decode())
        no_sensitive(canonical(row))
        if not line_roles(row, body) or line_roles(row, body) != item['roles']:
            raise ValueError('selected line role mismatch')
        parsed.append((item['start'], item['end'], row))
        offset = item['copy_end']
    if offset != len(raw):
        raise ValueError('unindexed selected bytes')
    return selected_facts(segment, parsed, body), parsed
