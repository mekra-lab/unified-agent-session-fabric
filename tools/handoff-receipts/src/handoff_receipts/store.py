"""Sealed intent plus atomic, hash-linked observation checkpoints.

CURRENT is replaced only after its immutable checkpoint has reached disk. A
crash can lose the most recent observation, never the pre-dispatch intent.
"""
import fcntl
import os
import re
import shutil
import tempfile
import uuid
from contextlib import contextmanager
from pathlib import Path
from .common import (canonical, digest, fsync_dir, no_sensitive, now, private_dir,
                     read_json, write_bytes, write_json)

CORE = ('source.json', 'request.json', 'body.txt', 'INTERPRETATION.md')


def default_root():
    return Path(os.environ.get('XDG_DATA_HOME', Path.home() / '.local/share')) / 'handoff-receipts'


class Store:
    def __init__(self, root=None):
        self.warnings = []
        self.root = Path(root or default_root()).absolute()
        if self.root != self.root.resolve():
            raise ValueError('symlink store refused')

    def path(self, attempt):
        if not re.fullmatch(r'[0-9a-f]{32}', attempt):
            raise ValueError('expected a full attempt ID')
        return self.root / attempt

    def create(self, instruction, request, retry_of=None, name=None):
        instruction = Path(instruction).resolve()
        body = instruction.read_bytes()
        no_sensitive(body.decode('utf-8'))
        no_sensitive(str(instruction))
        no_sensitive(canonical(request))
        no_sensitive(name or '')
        engine = request.get('engine', 'codex')
        if engine not in ('codex', 'claude', 'agy'):
            raise ValueError('unsupported engine')
        profile = {'claude': 'claude-selected-lines-v1', 'agy': 'agy-selected-fields-v1'}.get(engine, 'codex-selected-lines-v2')
        parent = self.load(retry_of)[0] if retry_of else None
        if parent and parent.get('engine', 'codex') != engine:
            raise ValueError('retry must use the same engine')
        if parent and parent['sha256'] != digest(body):
            raise ValueError('retry must use identical instruction bytes')
        private_dir(self.root)
        attempt = uuid.uuid4().hex
        # Hidden staging names never appear in ids(). Nothing is dispatchable
        # until the entire sealed intent and initial checkpoint are published.
        folder = Path(tempfile.mkdtemp(prefix='.pending-' + attempt + '-', dir=self.root))
        try:
            private_dir(folder / 'states')
            source = {'bundle_version': 1, 'interpretation_version': 2, 'attempt_id': attempt,
                      'engine': engine, 'interpretation_profile': profile,
                      'path': str(instruction), 'sha256': digest(body), 'size': len(body),
                      'mtime_ns': instruction.stat().st_mtime_ns, 'captured_at': now(),
                      'retry_of': retry_of, 'group': parent['group'] if parent else attempt, 'name': name}
            write_bytes(folder / 'body.txt', body)
            write_json(folder / 'source.json', source)
            write_json(folder / 'request.json', request)
            write_bytes(folder / 'INTERPRETATION.md', Path(__file__).with_name('INTERPRETATION.md').read_bytes())
            write_json(folder / 'manifest.json', {'version': 1, 'files': {
                name: digest((folder / name).read_bytes()) for name in CORE}})
            self._save(folder, {'control': [{'event': 'intent_recorded', 'at': source['captured_at']}],
                                'target': {'observation_version': 2, 'segments': [], 'issues': []}})
            fsync_dir(folder)
            os.rename(folder, self.path(attempt))
            fsync_dir(self.root)
        finally:
            if folder.exists():
                shutil.rmtree(folder)
        return attempt

    @contextmanager
    def lock(self, attempt):
        path = self.path(attempt) / '.lock'
        fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise ValueError('attempt is still being captured or recovered') from None
            yield
        finally:
            os.close(fd)

    def save(self, attempt, state):
        self._save(self.path(attempt), state)

    @staticmethod
    def _save(folder, state):
        head = folder / 'CURRENT'
        previous = head.read_text().strip() if head.exists() else None
        checkpoint = {'version': 1, 'previous': previous, 'state': state}
        encoded = (canonical(checkpoint) + '\n').encode()
        no_sensitive(encoded.decode())
        sha = digest(encoded)
        write_bytes(folder / 'states' / (sha + '.json'), encoded)
        write_bytes(head, (sha + '\n').encode())

    def load(self, attempt):
        folder = self.path(attempt)
        if folder != folder.resolve() or any(p.is_symlink() for p in folder.rglob('*')):
            raise ValueError('symlink bundle refused')
        allowed = set(CORE) | {'manifest.json', 'CURRENT', 'states', '.lock'}
        if any(p.name not in allowed and not p.name.startswith('.writing-') for p in folder.iterdir()):
            raise ValueError('unexpected bundle file')
        manifest = read_json(folder / 'manifest.json')
        if manifest.get('version') != 1 or set(manifest['files']) != set(CORE):
            raise ValueError('unsupported bundle manifest')
        for name in CORE:
            if digest((folder / name).read_bytes()) != manifest['files'][name]:
                raise ValueError('bundle integrity mismatch')
        # Check every checkpoint, including complete writes orphaned by a crash.
        for path in (folder / 'states').iterdir():
            if path.name.startswith('.writing-'):
                continue
            if path.name != digest(path.read_bytes()) + '.json':
                raise ValueError('checkpoint integrity mismatch')
        sha, seen, latest = (folder / 'CURRENT').read_text().strip(), set(), None
        while sha:
            if not re.fullmatch('[0-9a-f]{64}', sha) or sha in seen:
                raise ValueError('invalid checkpoint chain')
            seen.add(sha)
            item = read_json(folder / 'states' / (sha + '.json'))
            if item['version'] != 1:
                raise ValueError('unsupported checkpoint version')
            latest = item['state'] if latest is None else latest
            sha = item['previous']
        source, request = read_json(folder / 'source.json'), read_json(folder / 'request.json')
        body = (folder / 'body.txt').read_bytes()
        if source['attempt_id'] != attempt or source['sha256'] != digest(body):
            raise ValueError('source binding mismatch')
        if source['bundle_version'] != 1 or source['interpretation_version'] != 2:
            raise ValueError('unsupported bundle version')
        engine = source.get('engine', 'codex')
        if engine not in ('codex', 'claude', 'agy') or request.get('engine', 'codex') != engine:
            raise ValueError('bundle engine mismatch')
        profile = {'claude': 'claude-selected-lines-v1', 'agy': 'agy-selected-fields-v1'}.get(engine, 'codex-selected-lines-v2')
        if source.get('interpretation_profile', profile) != profile or request.get('interpretation_profile', profile) != profile:
            raise ValueError('unsupported engine interpretation profile')
        if engine == 'claude' and request.get('via') != 'exec':
            raise ValueError('unsupported Claude transport')
        if engine == 'agy' and request.get('via') != 'exec':
            raise ValueError('unsupported agy transport')
        return source, request, latest, body

    def ids(self):
        if not self.root.exists():
            return []
        return sorted(p.name for p in self.root.iterdir() if re.fullmatch('[0-9a-f]{32}', p.name))

    def forget(self, attempt):
        folder = self.path(attempt)
        # Integrity failure must not prevent explicit deletion. Never follow a
        # corrupt top-level symlink; rmtree also does not follow nested symlinks.
        if folder.is_symlink() or folder.is_file():
            folder.unlink()
        else:
            with self.lock(attempt):
                shutil.rmtree(folder)
        fsync_dir(self.root)

    def warn_skipped(self, attempt, error):
        warning = {'warning': 'broken_bundle_skipped', 'attempt_id': attempt,
                   'category': type(error).__name__}
        if warning not in self.warnings:
            self.warnings.append(warning)
