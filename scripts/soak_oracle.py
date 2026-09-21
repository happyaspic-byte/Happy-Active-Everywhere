"""Independent filesystem, database and elapsed-observation checks."""
from dataclasses import dataclass
import hashlib
import json
import math
from pathlib import Path
import sqlite3
import stat


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def file_value(data):
    return {'kind': 'file', 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}


def manifest(root):
    root = Path(root)
    assert stat.S_ISDIR(root.lstat().st_mode), 'data root is missing or replaced'
    result = {}
    def visit(directory):
        for path in sorted(directory.iterdir()):
            # Same reserved namespace as the engine; journals are checked via
            # separate state/evidence inspection, never treated as user files.
            if path.name.lower().startswith('.everywhere-'):
                continue
            name = path.relative_to(root).as_posix()
            info = path.lstat()
            if stat.S_ISDIR(info.st_mode):
                result[name] = {'kind': 'directory'}
                visit(path)
            else:
                assert stat.S_ISREG(info.st_mode), f'non-regular visible path: {name}'
                result[name] = {'kind': 'file', 'bytes': info.st_size, 'sha256': sha256(path)}
    visit(root)
    return result


def assert_manifest(root, expected):
    actual = manifest(root)
    assert actual == expected, {'root': str(root), 'expected': expected, 'actual': actual}


def index_snapshot(state, folder='soak'):
    path = Path(state) / 'shares' / folder / 'index.sqlite'
    assert path.is_file() and not path.is_symlink(), 'missing or unsafe index'
    with sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True, timeout=1) as db:
        db.execute('BEGIN')
        entries = {name: json.loads(versions) for name, versions in
                   db.execute('SELECT path,versions FROM entries ORDER BY path')}
        for versions in entries.values():
            versions['heads'].sort(key=lambda head: json.dumps(head, sort_keys=True))
        return {
            'entries': entries,
            'sequence': int(db.execute("SELECT value FROM meta WHERE key='seq'").fetchone()[0]),
            'pending': [row[0] for row in db.execute('SELECT path FROM pending ORDER BY path')],
            'incoming': [list(row) for row in db.execute('SELECT path,versions FROM incoming ORDER BY path')],
            'needed': [row[0] for row in db.execute('SELECT hash FROM needed ORDER BY hash')],
            'history_count': db.execute('SELECT count(*) FROM history').fetchone()[0],
        }


def assert_reconciled(snapshots):
    assert snapshots
    expected = snapshots[0]['entries']
    for snapshot in snapshots:
        assert snapshot['entries'] == expected, 'peer causal heads have not converged'
        assert not snapshot['pending'], 'unreviewed deletion remains'
        assert not snapshot['incoming'] and not snapshot['needed'], 'receive queues have not drained'


@dataclass
class Observation:
    target: float
    min_cycles: int
    gap_limit: float
    first: float | None = None
    last: float | None = None
    last_utc: float | None = None
    max_gap: float = 0
    valid: bool = True

    def __post_init__(self):
        assert math.isfinite(self.target) and self.target > 0
        assert self.min_cycles > 0 and self.gap_limit > 0

    def observe(self, monotonic, utc):
        assert self.valid, 'observation was already interrupted'
        if not all(math.isfinite(value) for value in [monotonic, utc]):
            self.valid = False
            raise AssertionError('non-finite observation time')
        if self.last is not None:
            gap = max(monotonic - self.last, abs(utc - self.last_utc))
            if monotonic < self.last or gap > self.gap_limit:
                self.valid = False
                raise AssertionError(f'observation gap exceeds continuity contract: {gap:.3f}s')
            self.max_gap = max(self.max_gap, gap)
        else:
            self.first = monotonic
        self.last, self.last_utc = monotonic, utc

    @property
    def elapsed(self):
        return 0 if self.first is None else self.last - self.first

    def complete(self, monotonic, cycles):
        return (self.valid and self.last is not None
                and 0 <= monotonic - self.last <= self.gap_limit
                and self.elapsed >= self.target and cycles >= self.min_cycles)
