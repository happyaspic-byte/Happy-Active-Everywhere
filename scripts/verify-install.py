"""Exercise installers using an actual build, or a disposable local fixture."""
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path)
args = parser.parse_args()
repo = Path(__file__).resolve().parent.parent
windows = os.name == 'nt'
name = 'everywhere.exe' if windows else 'everywhere'
with tempfile.TemporaryDirectory(prefix='everywhere-install-') as directory:
    base = Path(directory)
    prefix = base / 'installed with spaces'
    a, b = base / 'package-a', base / 'package-b'
    for package, version in [(a, 'smoke-a'), (b, 'smoke-b')]:
        package.mkdir()
        binary = package / name
        if args.binary:
            shutil.copy2(args.binary, binary)
        else:
            assert not windows, 'Windows installer verification requires a real binary'
            binary.write_text('#!/bin/sh\nprintf "everywhere installation-fixture\\n"\n')
            binary.chmod(0o755)
        (package / 'VERSION').write_text(version + '\n')
        digest = hashlib.sha256(binary.read_bytes()).hexdigest()
        (package / 'SHA256SUMS').write_text(f'{digest}  {name}\n')
    def install(action, package=None, expect_success=True):
        if windows:
            command = ['pwsh', '-NoProfile', '-File', str(repo / 'scripts/install.ps1'), '-Action', action, '-Prefix', str(prefix)]
            if package:
                command += ['-Package', str(package)]
        else:
            command = ['sh', str(repo / 'scripts/install.sh'), action]
            if package:
                command += [str(package)]
            command += [str(prefix)]
        result = subprocess.run(command, capture_output=True, text=True, timeout=30)
        assert (result.returncode == 0) == expect_success, (command, result.stdout, result.stderr)
    install('install', a)
    first = (prefix / 'current').read_text().strip()
    assert first.startswith('smoke-a-')
    if windows:
        launcher = ['pwsh', '-NoProfile', '-File', str(prefix / 'everywhere.ps1'), '--version']
    else:
        launcher = [str(prefix / 'everywhere'), '--version']
    result = subprocess.run(launcher, capture_output=True, text=True, timeout=20)
    assert result.returncode == 0 and 'everywhere' in result.stdout, result
    install('install', b)
    second = (prefix / 'current').read_text().strip()
    assert second != first and (prefix / 'previous').read_text().strip() == first
    install('rollback')
    assert (prefix / 'current').read_text().strip() == first
    assert (prefix / 'versions' / second / name).is_file()
    (b / 'SHA256SUMS').write_text('0' * 64 + f'  {name}\n')
    install('install', b, expect_success=False)
    assert (prefix / 'current').read_text().strip() == first
    assert (prefix / 'versions' / first / name).is_file()
print('Installer acceptance: install, launcher, upgrade, rollback and corrupt-package rejection passed.')
