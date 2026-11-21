#!/usr/bin/env python3
"""Synthetic Codex process. Never calls a model or a real daemon."""
import json
import os
import signal
import base64
import subprocess
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

sys.stderr.buffer.write(bytes.fromhex(os.environ.get('FAKE_CHILD_STDERR_HEX', '')))
sys.stderr.buffer.flush()

args = sys.argv[1:]
if '--version' in args:
    print('codex-cli synthetic')
    raise SystemExit(0)
mode = os.environ.get('FAKE_MODE', 'completed')
root = Path(os.environ['FAKE_SESSIONS'])
root.mkdir(parents=True, exist_ok=True)
body = sys.stdin.read()
thread = args[2] if args[:2] == ['exec', 'resume'] else str(uuid.uuid4())
turn = str(uuid.uuid4())
path = root / ('rollout-' + thread + '.jsonl')
buffered = []

def row(kind, payload):
    timestamp = datetime.now(timezone.utc).isoformat()
    if os.environ.get('FAKE_UTC_Z'):
        timestamp = timestamp.replace('+00:00', 'Z')
    encoded = json.dumps({'timestamp': timestamp, 'type': kind, 'payload': payload}) + '\n'
    if mode == 'delayed':
        buffered.append(encoded)
        return
    if mode in ('reported_only', 'reported_sensitive', 'reported_oversized'):
        return
    with path.open('a') as out:
        out.write(encoded)
        out.flush()
        os.fsync(out.fileno())

def event(kind, **fields):
    print(json.dumps({'type': kind, **fields}), flush=True)

if not path.exists():
    row('session_meta', {'id': thread, 'cwd': str(Path.cwd()), 'account_id': 'SYNTHETIC_PRIVATE_ACCOUNT'})
if mode != 'loss':
    event('thread.started', thread_id=thread)
if mode == 'missing':
    raise SystemExit(0)
if mode != 'missing_start':
    row('event_msg', {'type': 'task_started', 'turn_id': turn})
row('world_state', {'text': 'OMITTED_PRIVATE_WORLD private@example.invalid'})
row('response_item', {'type': 'message', 'role': 'user', 'content': [{'type': 'input_text', 'text': 'Injected AGENTS: OMITTED_PRIVATE_CONTEXT private@example.invalid'}]})
row('response_item', {'type': 'message', 'role': 'user', 'content': [{'type': 'input_text', 'text': 'Other input' if mode == 'missing_input' else body}], **({'private': 'private@example.invalid'} if mode == 'sensitive_input' else {})})
if mode.startswith('compacted'):
    row('compacted', {'replacement_history': 'COMPACTED_PRIVATE_CONTEXT private@example.invalid ' + 'x' * 1152000})
if mode != 'loss':
    event('turn.started', **({} if mode.startswith('compacted') else {'turn_id': turn}))
if mode == 'wait':
    def stop(sig, frame):
        row('event_msg', {'type': 'turn_aborted', 'turn_id': turn})
        raise SystemExit(130)
    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)
    time.sleep(60)
if mode in ('failed', 'failed_complete'):
    row('event_msg', {'type': 'task_failed', 'turn_id': turn} if mode == 'failed' else
        {'type': 'task_complete', 'turn_id': turn, 'error': {'message': 'Synthetic invalid model'}, 'last_agent_message': None})
    event('turn.failed', error={'message': 'private@example.invalid'})
    raise SystemExit(1)
text = 'private@example.invalid' if mode in ('sensitive_final', 'reported_sensitive') else 'ACK'
row('response_item', {'type': 'message', 'role': 'assistant', 'phase': 'final_answer', 'content': [{'type': 'output_text', 'text': text}]})
if mode == 'stream_conflict':
    row('event_msg', {'type': 'task_failed', 'turn_id': turn})
else:
    row('event_msg', {'type': 'task_complete', 'turn_id': 'other-turn' if mode == 'compacted_wrong_turn' else turn, 'last_agent_message': text})
if mode != 'loss':
    event('item.completed', item={'id': 'final-1', 'type': 'agent_message', 'text': text})
    if mode == 'reported_oversized':
        event('item.completed', item={'id': 'final-2', 'type': 'agent_message', 'text': 'x' * (4 * 1024 * 1024 + 100)})
    event('turn.completed')
if mode == 'delayed':
    encoded = base64.b64encode(''.join(buffered).encode()).decode()
    subprocess.Popen([sys.executable, '-c',
        'import time,sys,base64; from pathlib import Path; time.sleep(.25); Path(sys.argv[1]).write_bytes(base64.b64decode(sys.argv[2]))',
        str(path), encoded], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
