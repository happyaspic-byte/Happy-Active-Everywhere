"""Package a locally built executable and its matching operational documents."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import tomllib
import zipfile

root = Path(__file__).resolve().parent.parent
version = tomllib.loads((root / 'Cargo.toml').read_text())['package']['version']
commit = os.environ.get('GITHUB_SHA', 'local')
label = f'{version}-{commit[:12]}'
name = 'everywhere.exe' if platform.system() == 'Windows' else 'everywhere'
binary = root / 'target' / 'release' / name
assert binary.is_file(), f'Missing release binary: {binary}'
folder = root / 'dist' / f'everywhere-{label}-{platform.system().lower()}-{platform.machine().lower()}'
folder.mkdir(parents=True, exist_ok=False)
shutil.copy2(binary, folder / name)
shutil.copy2(root / 'README.md', folder / 'README.md')
shutil.copytree(root / 'docs', folder / 'docs')
if (root / 'folder-report').exists():
    shutil.copytree(root / 'folder-report', folder / 'verification')
(folder / 'scripts').mkdir()
for script in ['install.sh', 'install.ps1']:
    shutil.copy2(root / 'scripts' / script, folder / 'scripts' / script)
(folder / 'VERSION').write_text(label + '\n')
digest = hashlib.sha256(binary.read_bytes()).hexdigest()
(folder / 'SHA256SUMS').write_text(f'{digest}  {name}\n')
(folder / 'build.json').write_text(json.dumps({'version':version,'commit':commit,'platform':platform.platform(),'machine':platform.machine(),'sha256':digest,'classification':'alpha'},indent=2) + '\n')
archive = folder.parent / (folder.name + '.zip')
with zipfile.ZipFile(archive,'w',compression=zipfile.ZIP_DEFLATED) as out:
    for item in sorted(folder.rglob('*')):
        if item.is_file():
            out.write(item, Path(folder.name) / item.relative_to(folder))
archive.with_name(archive.name + '.sha256').write_text(f'{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n')
print(f'Packaged {archive.name}; executable SHA-256 {digest}')
