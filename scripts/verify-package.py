"""Verify package contents with disposable fixtures, including secret sentinels."""
import hashlib
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import zipfile

source = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='everywhere-package-test-') as directory:
    root = Path(directory)
    (root / 'scripts').mkdir()
    shutil.copy2(source / 'scripts/package.py', root / 'scripts/package.py')
    for name in ['install.sh', 'install.ps1']:
        (root / 'scripts' / name).write_text('fixture installer\n')
    (root / 'Cargo.toml').write_text('[package]\nversion = "0.0.0-test"\n')
    (root / 'README.md').write_text('fixture readme\n')
    (root / 'docs').mkdir()
    (root / 'docs/device-backup.md').write_text('fixture operator documentation\n')
    (root / 'target/release').mkdir(parents=True)
    name = 'everywhere.exe' if platform.system() == 'Windows' else 'everywhere'
    (root / 'target/release' / name).write_bytes(b'fixture executable')
    reports = root / 'folder-report'
    fixtures = reports / 'recovery-evidence' / 'device-fixture'
    fixtures.mkdir(parents=True)
    sentinel = b'TEST-ONLY-PRIVATE-FIXTURE-MUST-NOT-BE-DISTRIBUTED'
    for file in ['identity.key.der', 'offline.agekey', 'backup.age', 'index.sqlite', 'test.dmg']:
        (fixtures / file).write_bytes(sentinel)
    (reports / 'unreviewed.json').write_bytes(sentinel)
    (reports / 'delta.json').write_text('{"sha256_mismatches":0}\n')
    result = subprocess.run([sys.executable, str(root / 'scripts/package.py')],
                            env={**os.environ, 'GITHUB_SHA': 'test'},
                            capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stderr
    archive, = (root / 'dist').glob('*.zip')
    with zipfile.ZipFile(archive) as package:
        names = package.namelist()
        checksum_file, = [n for n in names if n.endswith('/SHA256SUMS')]
        checksum, executable = package.read(checksum_file).decode().strip().split()
        expected_name = 'everywhere.exe' if platform.system() == 'Windows' else 'everywhere'
        assert executable == expected_name, 'checksum must name the actual executable'
        assert checksum == hashlib.sha256(b'fixture executable').hexdigest()
        assert any(n.endswith('/docs/device-backup.md') for n in names)
        assert any(n.endswith('/verification/delta.json') for n in names)
        for name in names:
            assert 'recovery-evidence' not in name, f'private test fixture packaged: {name}'
            assert not name.endswith('/unreviewed.json'), 'unreviewed report packaged'
            assert sentinel not in package.read(name), f'private fixture bytes packaged: {name}'
    print('Package acceptance: executable, docs and approved summaries only; private fixtures excluded.')
