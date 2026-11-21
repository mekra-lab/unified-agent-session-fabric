"""Bounded native observations and exact-ID daemon polling."""
import json
import os
import time
from pathlib import Path
from .common import TERMINAL, status, now
from .g4_helpers import rpc_error_body
from .diagnostics import ObservationError

def turn_list(client, thread):
    try:
        result = client.call('thread/turns/list', {'threadId': thread, 'sortDirection': 'asc', 'limit': 100})
    except RuntimeError as error:
        body = rpc_error_body('thread/turns/list', error)
        if body and 'not materialized' in str(body.get('message', '')).lower():
            return []
        raise
    if result.get('nextCursor') or not isinstance(result.get('data'), list):
        raise ValueError('turn list pagination/shape unsupported; no inference')
    return [{'id': t['id'], 'status': status(t.get('status'))} for t in result['data']]


def wait_exact(client, thread, turn, timeout=300):
    deadline = time.monotonic() + timeout
    observed = None
    while time.monotonic() < deadline:
        matches = [t for t in turn_list(client, thread) if t['id'] == turn]
        if len(matches) > 1:
            raise ValueError('ambiguous exact turn ID')
        if matches:
            observed = {**matches[0], 'observed_at': now(), 'method': 'thread/turns/list_exact_id'}
            if observed['status'] in TERMINAL:
                return observed
        time.sleep(min(.1, max(0, deadline - time.monotonic())))
    return observed


def read_snapshot(path):
    """Preflight requires only bounded bytes, not materialized JSON/session meta."""
    path = Path(path)
    if path.absolute() != path.resolve():
        raise ObservationError('rollout_unsafe_path')
    if not path.is_file():
        path.stat()  # Preserve FileNotFoundError for a new unmaterialized thread.
        raise ObservationError('rollout_unsafe_path')
    with path.open('rb') as stream:
        st = os.fstat(stream.fileno())
        if st.st_size > 32 * 1024 * 1024:
            raise ObservationError('rollout_too_large', snapshot_bytes=st.st_size)
        return stream.read(st.st_size)


def read_native(path, thread):
    content = read_snapshot(path)
    # An incomplete trailing append is outside this observation.
    content = content[:content.rfind(b'\n') + 1]
    parsed, offset = [], 0
    for number, line in enumerate(content.splitlines(keepends=True), 1):
        try:
            row = json.loads(line)
        except (ValueError, UnicodeError):
            raise ObservationError('rollout_invalid_json', line=number, line_bytes=len(line)) from None
        if not isinstance(row, dict) or not isinstance(row.get('payload', {}), dict):
            raise ObservationError('rollout_invalid_shape', line=number)
        parsed.append((offset, offset + len(line), row))
        offset += len(line)
    if not parsed:
        raise ObservationError('rollout_incomplete')
    if parsed[0][2].get('type') != 'session_meta':
        raise ObservationError('rollout_metadata_missing')
    meta = parsed[0][2]['payload']
    if meta.get('session_id', meta.get('id')) != thread:
        raise ObservationError('rollout_thread_mismatch')
    return content, parsed
