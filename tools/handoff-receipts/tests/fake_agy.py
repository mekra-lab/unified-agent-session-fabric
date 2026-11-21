#!/usr/bin/env python3
"""Invented Antigravity producer. No product decoder/encoder imports."""
import json
import os
import signal
import sqlite3
import subprocess
import sys
import time
import uuid
from pathlib import Path

sys.stderr.buffer.write(bytes.fromhex(os.environ.get('FAKE_CHILD_STDERR_HEX', '')))
sys.stderr.buffer.flush()


def vi(value):
    encoded = bytearray()
    while value > 127:
        encoded.append((value & 127) | 128)
        value >>= 7
    encoded.append(value)
    return bytes(encoded)


def number(field, value):
    return vi(field << 3) + vi(value)


def blob(field, value):
    value = value.encode('utf-8') if isinstance(value, str) else value
    return vi((field << 3) | 2) + vi(len(value)) + value


DDL = '''
CREATE TABLE IF NOT EXISTS trajectory_meta (trajectory_id TEXT PRIMARY KEY, cascade_id TEXT);
CREATE TABLE IF NOT EXISTS steps (idx INTEGER PRIMARY KEY, step_type INTEGER, status INTEGER, metadata BLOB, step_payload BLOB);
CREATE TABLE IF NOT EXISTS executor_metadata (idx INTEGER PRIMARY KEY, data BLOB);
CREATE TABLE IF NOT EXISTS gen_metadata (idx INTEGER PRIMARY KEY, data BLOB);
'''


def connect(path, sid):
    c = sqlite3.connect(path)
    c.execute('PRAGMA journal_mode=WAL')
    c.executescript(DDL)
    if not c.execute('SELECT count(*) FROM trajectory_meta').fetchone()[0]:
        c.execute('INSERT INTO trajectory_meta VALUES (?, ?)', (str(uuid.uuid4()), sid))
    c.commit()
    return c


def write_rows(path, sid, index, execution, body, mode):
    c = connect(path, sid)
    meta = blob(12, execution) + blob(35, 'UNSELECTED-METADATA-SECRET')
    content = body + 'different' if mode == 'wrong_body' else body
    user = blob(2, content) + blob(12, 'fictional.person@example.invalid')
    payload = number(1, 14) + number(4, 3) + blob(5, meta) + blob(19, user)
    if mode == 'unknown_fields':
        payload += blob(900, 'UNSELECTED-UNKNOWN-SECRET') + number(901, 5)
    if mode == 'duplicate_field':
        payload += blob(19, blob(2, body))
    if mode == 'wrong_wire':
        payload = number(1, 14) + number(4, 3) + blob(5, meta) + blob(19, number(2, 1))
    if mode == 'corrupt_blob':
        payload += b'\x80'
    c.execute('INSERT INTO steps VALUES (?, ?, ?, ?, ?)',
              (index, 15 if mode == 'sql_mismatch' else 14, 3, meta, payload))
    response = 'AGY-ACK' if mode == 'lf_difference' else 'AGY-ACK\n'
    if mode in ('sensitive_response', 'sensitive_late_cancel'):
        response = 'fictional.person@example.invalid'
    planner = number(1, 15) + number(4, 3) + blob(5, meta) + blob(20, blob(1, response) + blob(8, response))
    c.execute('INSERT INTO steps VALUES (?, 15, 3, ?, ?)', (index + 1, meta, planner))
    if mode == 'ambiguous_native':
        c.execute('INSERT INTO steps VALUES (?, 14, 3, ?, ?)', (index + 2, meta, payload))
    canceled = mode in ('interrupted', 'timeout', 'success_timeout', 'conflict', 'sigint')
    executor = number(1, 2 if canceled else 4) + blob(9, execution) + blob(3, 'UNSELECTED-CONFIG-SECRET')
    ei = c.execute('SELECT coalesce(max(idx), -1) + 1 FROM executor_metadata').fetchone()[0]
    if mode not in ('late_cancel', 'sensitive_late_cancel'):
        c.execute('INSERT INTO executor_metadata VALUES (?, ?)', (ei, executor))
    c.execute('INSERT OR REPLACE INTO gen_metadata VALUES (0, ?)', (b'UNSELECTED-GEN-SECRET',))
    c.commit()
    return c  # Caller may keep the WAL alive through the reader's transaction.


def emit(kind, sid, **value):
    record = {'event': kind, 'conversation_id': sid} if kind == 'init' else {'event': kind, kind: {'conversation_id': sid, **value}}
    if kind == 'init':
        record['init'] = {'account': 'NEVER-RETAIN-INIT', 'tools': ['fake_private_tool']}
    print(json.dumps(record, ensure_ascii=True), flush=True)


def main():
    mode = os.environ.get('FAKE_AGY_MODE', 'normal')
    args = sys.argv[1:]
    if args == ['--version']:
        print(os.environ.get('FAKE_AGY_VERSION', '1.5.9'))
        return 0
    if '--executor-commit' in args:
        time.sleep(.3)
        c = sqlite3.connect(args[1])
        c.execute('INSERT INTO executor_metadata VALUES (0, ?)', (number(1, 2) + blob(9, args[2]),))
        c.commit()
        c.close()
        return 0
    if '--delayed-writer' in args:
        path, sid, index, execution, body = json.loads(os.environ['FAKE_AGY_DELAYED'])
        c = connect(path, sid)
        # Hold a real writer lock in rollback mode to exercise SQLITE_BUSY.
        if mode == 'busy':
            c.execute('PRAGMA journal_mode=DELETE')
            c.execute('BEGIN EXCLUSIVE')
        Path(os.environ['FAKE_AGY_READY']).write_text('ready')
        # Hold the lock (busy) or delay the commit (delayed_commit) until the test
        # signals that its first observation is done, so a slow host cannot let the
        # writer finish before that observation.
        release = Path(os.environ['FAKE_AGY_READY'] + '.release')
        deadline = time.monotonic() + 30
        while not release.exists() and time.monotonic() < deadline:
            time.sleep(.01)
        c.rollback()
        c.close()
        write_rows(path, sid, index, execution, body, 'normal').close()
        return 0
    trace = os.environ.get('FAKE_AGY_TRACE')
    if trace:
        with open(trace, 'a') as out:
            out.write(json.dumps(args) + '\n')
    assert '-p' not in args
    assert args[args.index('--input-format') + 1] == 'stream-json'
    assert args[args.index('--output-format') + 1] == 'stream-json'
    data = sys.stdin.buffer.read().decode('utf-8')
    record = json.loads(data)
    assert record['event'] == 'user'
    body = record['message']['content']
    root = Path(os.environ['FAKE_AGY_ROOT'])
    root.mkdir(parents=True, exist_ok=True)
    sid = args[args.index('--conversation') + 1] if '--conversation' in args else str(uuid.uuid4())
    if mode == 'wrong_id':
        sid = str(uuid.uuid4())
    if mode == 'no_id':
        print(json.dumps({'event': 'error', 'error': 'synthetic failure'}), flush=True)
        return 1
    path = root / (sid + '.db')
    c = connect(path, sid)
    index = c.execute('SELECT coalesce(max(idx), -1) + 1 FROM steps').fetchone()[0]
    c.close()
    execution = str(uuid.uuid4())
    if mode == 'hash_change':
        with Path(__file__).open('a') as out:
            out.write('\n# synthetic auto-update\n')
    if mode not in ('busy', 'delayed_commit'):
        c = write_rows(path, sid, index, execution, body, mode)
        if mode in ('late_cancel', 'sensitive_late_cancel'):
            subprocess.Popen([sys.executable, str(Path(__file__).resolve()), '--executor-commit', str(path), execution],
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    else:
        ready = root / ('ready-' + sid)
        env = dict(os.environ, FAKE_AGY_DELAYED=json.dumps([str(path), sid, index, execution, body]), FAKE_AGY_READY=str(ready))
        subprocess.Popen([sys.executable, str(Path(__file__).resolve()), '--delayed-writer'], env=env,
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.monotonic() + 5
        while not ready.exists() and time.monotonic() < deadline:
            time.sleep(.01)
    emit('init', sid)
    if mode == 'wal':
        # Keep this separate process's connection open after the CLI exits.
        # A reader holding WAL state is enough; no native product reads occur here.
        subprocess.Popen([sys.executable, '-c', 'import sqlite3,sys,time;c=sqlite3.connect(sys.argv[1]);c.execute("BEGIN");c.execute("SELECT * FROM steps").fetchall();time.sleep(1);c.close()', str(path)],
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(.05)
    if mode == 'sigint':
        interrupted = False

        def caught(signum, frame):
            nonlocal interrupted
            interrupted = True
        signal.signal(signal.SIGINT, caught)
    if mode != 'no_input_index':
        emit('step_update', sid, step_type='user_input', step_index=index + 10 if mode == 'wrong_index' else index, state='DONE')
        if mode == 'ambiguous_index':
            emit('step_update', sid, step_type='user_input', step_index=index + 1, state='DONE')
        if mode == 'repeated_index':
            emit('step_update', sid, step_type='user_input', step_index=index, state='DONE')
    if mode == 'missing_input_id':
        print(json.dumps({'event': 'step_update', 'step_update': {'step_type': 'user_input', 'step_index': index + 1}}), flush=True)
    elif mode == 'invalid_index_after':
        emit('step_update', sid, step_type='user_input', step_index='invalid', state='DONE')
    elif mode == 'stream_incomplete':
        print('{"event":', flush=True)
    if mode == 'sigint':
        deadline = time.monotonic() + 5
        while not interrupted and time.monotonic() < deadline:
            time.sleep(.01)
    if mode == 'different_stream_id':
        emit('step_update', str(uuid.uuid4()), step_type='agent_response', step_index=index + 1, state='DONE')
    if mode == 'malformed_result':
        print('{"event":"result",', flush=True)
    elif mode != 'no_result':
        error = mode in ('interrupted', 'timeout', 'sigint', 'failed')
        result = {'status': 'ERROR' if error else 'SUCCESS',
                  'response': 'fictional.person@example.invalid' if mode == 'sensitive_result' else 'AGY-ACK\n',
                  'num_turns': 999}
        if mode in ('timeout', 'success_timeout'):
            result['finished_reason'] = 'print_timeout'
        elif mode in ('interrupted', 'sigint'):
            result['finished_reason'] = 'interrupted'
        elif mode == 'failed':
            result['finished_reason'] = 'other'
        elif mode == 'null_reason':
            result['finished_reason'] = None
        if mode == 'success_timeout':
            result['response'] = ''
        elif mode == 'invalid_utf8':
            result['response'] = '\ud800'
        emit('result', sid, **result)
        if mode == 'multiple_results':
            emit('result', sid, status='SUCCESS', response='AGY-ACK\n')
    # WAL test deliberately leaves the native keeper alive for a bounded time.
    if mode not in ('busy', 'delayed_commit'):
        c.close()
    return 3 if mode == 'timeout' else 1 if mode in ('interrupted', 'sigint', 'failed') else 0


if __name__ == '__main__':
    raise SystemExit(main())
