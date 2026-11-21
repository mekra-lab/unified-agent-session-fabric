"""Private, durable files and conservative privacy checks."""
import hashlib
import json
import os
import re
import tempfile
from datetime import datetime, timezone
from pathlib import Path

TERMINAL = {'completed', 'failed', 'interrupted', 'cancelled', 'aborted'}


def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'))


def digest(value):
    return hashlib.sha256(value.encode() if isinstance(value, str) else value).hexdigest()


def now():
    return datetime.now(timezone.utc).isoformat()


def status(value):
    return value.get('type') if isinstance(value, dict) else value


def no_sensitive(text):
    if re.search(r'[\w.+-]+@[\w.-]+\.[A-Za-z]{2,}|\bsk-[A-Za-z0-9_-]+|(?i:Bearer\s+\S+)|(?i:(?:access_token|refresh_token|account(?:_id)?|api_key|authorization|email)\s*["\x27:=])', text):
        raise ValueError('sensitive content refused before persistence')


def private_dir(path):
    path = Path(path).absolute()
    if path != path.resolve():
        raise ValueError('symlink path refused')
    missing, parent = [], path
    while not parent.exists():
        missing.append(parent)
        parent = parent.parent
    for directory in reversed(missing):
        directory.mkdir(mode=0o700)
    # Do not chmod ancestors owned by the caller; only this requested directory.
    os.chmod(path, 0o700)
    return path


def fsync_dir(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def write_bytes(path, value):
    path = Path(path)
    if path.is_symlink() or path.parent.absolute() != path.parent.resolve():
        raise ValueError('symlink path refused')
    fd, tmp = tempfile.mkstemp(prefix='.writing-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as out:
            out.write(value)
            out.flush()
            os.fsync(out.fileno())
        os.replace(tmp, path)
        fsync_dir(path.parent)
    finally:
        Path(tmp).unlink(missing_ok=True)


def write_json(path, value):
    no_sensitive(canonical(value))
    write_bytes(path, (canonical(value) + '\n').encode())


def read_json(path):
    return json.loads(Path(path).read_text())


def append(path, value):
    # Used only by the synthetic daemon fixture.
    with Path(path).open('a') as out:
        out.write(canonical(value) + '\n')
        out.flush()
        os.fsync(out.fileno())


def parse_timestamp(value):
    """ISO-8601 UTC suffix support, including Python 3.10."""
    return datetime.fromisoformat(value[:-1] + '+00:00' if value.endswith('Z') else value)


def dispatch_facts(via, control):
    """Intent alone is not an attempt. A write-ahead send boundary may precede I/O."""
    events = {e['event'] for e in control}
    if via == 'daemon':
        sent = any(e['event'] == 'rpc_request' and e.get('method') == 'turn/start' for e in control)
        return {'attempted': sent, 'dispatched': sent}
    if 'process_started' in events:
        return {'attempted': True, 'dispatched': True}
    if 'dispatch_started' in events and 'process_error' not in events:
        # A killed wrapper may have launched the process without recording its PID.
        return {'attempted': True, 'dispatched': 'unknown'}
    return {'attempted': False, 'dispatched': False}
