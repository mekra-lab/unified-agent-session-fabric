#!/usr/bin/env python3
"""Invented Claude-shaped records, never copied from real transcripts."""
import json
import os
import sys
import uuid
from pathlib import Path

sys.stderr.buffer.write(bytes.fromhex(os.environ.get('FAKE_CHILD_STDERR_HEX', '')))
sys.stderr.buffer.flush()

args = sys.argv[1:]
mode = os.environ.get('FAKE_CLAUDE_MODE', 'completed')
sid = args[args.index('--resume') + 1] if '--resume' in args else args[args.index('--session-id') + 1]
root = Path(os.environ['FAKE_CLAUDE_PROJECTS']) / str(Path.cwd()).replace('/', '-')
root.mkdir(parents=True, exist_ok=True)
path = root / (sid + '.jsonl')
body = sys.stdin.read()
prompt = str(uuid.uuid4())
turn = os.environ.get('FAKE_USER_UUID', str(uuid.uuid4()))
previous = None

def row(kind, **fields):
    global previous
    record = {'type': kind, 'sessionId': sid, 'timestamp': '2026-01-02T03:04:05Z', **fields}
    if kind in ('user', 'assistant', 'attachment'):
        record.setdefault('uuid', str(uuid.uuid4()))
        record.setdefault('parentUuid', previous)
        previous = record['uuid']
    if mode != 'missing_native':
        with path.open('a') as out:
            out.write(json.dumps(record) + '\n')
    return record


def event(kind, **fields):
    print(json.dumps({'type': kind, 'session_id': sid, **fields}), flush=True)

stream_sid = str(uuid.uuid4()) if mode == 'identity_mismatch' else sid
event('system', subtype='init', session_id=stream_sid, apiKeySource='FAKE_AUTH_NOT_TO_PERSIST', organizationUuid='FAKE_ORG_NOT_TO_PERSIST')
row('queue-operation', operation='enqueue')
row('user', uuid=turn, promptId=prompt, turnPosition={'promptIndex': 0, 'turnIndex': 1},
    message={'role': 'user', 'content': body}, **({'email': 'fictional.person@example.invalid'} if mode == 'sensitive_input' else {}))
row('attachment', attachment={'type': 'session_context', 'context': {'userEmail': 'fictional.person@example.invalid', 'organizationUuid': 'FAKE_ORG_NOT_TO_PERSIST'}})
row('api-request-blob', message={'role': 'system', 'content': [{'type': 'text', 'text': 'FAKE_PRIVATE_CONTEXT'}]})
if mode == 'multiple_assistant':
    row('assistant', promptId=prompt, message={'role': 'assistant', 'content': [{'type': 'text', 'text': 'Interim reasoning.'}, {'type': 'tool_use', 'id': 'fake-tool', 'name': 'Read', 'input': {'file_path': 'fictional.txt'}}], 'stop_reason': 'tool_use'})
    row('user', promptId=prompt, message={'role': 'user', 'content': [{'type': 'tool_result', 'tool_use_id': 'fake-tool', 'content': 'fictional tool output'}]})
text = 'CLAUDE-ACK'
if mode in ('synthetic_error', 'native_error_report_completed', 'error_result_lost'):
    row('assistant', promptId=prompt, isApiErrorMessage=True, message={'role': 'assistant', 'model': '<synthetic>', 'content': [{'type': 'text', 'text': 'Invented provider failure.'}], 'stop_reason': 'stop_sequence'})
else:
    row('assistant', **({} if mode == 'parent_only' else {'promptId': prompt}), message={'role': 'assistant', 'model': 'synthetic-model', 'content': [{'type': 'text', 'text': 'fictional.person@example.invalid' if mode == 'sensitive_assistant' else text}], 'stop_reason': None if mode == 'interrupted' else 'end_turn'})
if mode in ('interrupted', 'foreign_interruption', 'interruption_result_lost', 'native_interruption_report_completed'):
    row('user', promptId=str(uuid.uuid4()) if mode == 'foreign_interruption' else prompt,
        message={'role': 'user', 'content': '[Request interrupted by user]'})
if mode == 'ambiguous_input':
    row('user', promptId=str(uuid.uuid4()), message={'role': 'user', 'content': body})
row('cost-state', totalCostUSD=0)
if mode not in ('result_lost', 'error_result_lost', 'interruption_result_lost'):
    error = mode in ('synthetic_error', 'interrupted')
    reason = 'api_error' if mode == 'synthetic_error' else 'aborted_streaming' if mode == 'interrupted' else 'completed'
    event('result', session_id=stream_sid, subtype='success', is_error=error, terminal_reason=reason,
          result='fictional.person@example.invalid' if mode == 'sensitive_result' else text)
    if mode == 'duplicate_result':
        event('result', is_error=False, terminal_reason='completed', result=text)
raise SystemExit(1 if mode == 'synthetic_error' else 0)
