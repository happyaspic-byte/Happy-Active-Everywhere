"""Exercise the real per-user service manager with disposable CLI/TLS peers."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--report', type=Path, required=True)
args = parser.parse_args()
binary = args.binary.resolve()
repo = Path(__file__).resolve().parent.parent
base = Path(tempfile.mkdtemp(prefix='everywhere-native-service-'))
commands = []
server = None
installed = False
failure = None
state = base / "a-state α $HOME %USERNAME% 'quote'"

def run(argv, success=True):
    result = subprocess.run([str(x) for x in argv], capture_output=True, text=True, timeout=150)
    secret = 'management-token' in argv
    commands.append({'args': [str(x) for x in argv], 'returncode': result.returncode,
                     'stdout': '[credential omitted]' if secret else result.stdout,
                     'stderr': result.stderr})
    assert (result.returncode == 0) == success, (argv, result.stdout if not secret else '', result.stderr)
    return result.stdout.strip()

def cli(*argv, success=True):
    return run([binary, *argv], success)

def service(action, *extra, success=True):
    value = cli('service', action, '--state', state, *extra, success=success)
    return json.loads(value) if success else value

def wait(predicate, seconds=35):
    deadline = time.monotonic() + seconds
    while not predicate():
        assert time.monotonic() < deadline, 'acceptance condition timed out'
        time.sleep(0.15)

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None

def payload(path, text):
    path.write_bytes(text.encode())
    return hashlib.sha256(text.encode()).hexdigest()

def unused_port():
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0))
        return probe.getsockname()[1]

def api(path):
    try:
        request = urllib.request.Request(f'http://127.0.0.1:{port}{path}', headers={'Authorization': f'Bearer {token}'})
        with urllib.request.urlopen(request, timeout=2) as response:
            return json.load(response)
    except (OSError, ValueError):
        return None

def process_alive(pid):
    if os.name == 'nt':
        result = subprocess.run(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command',
            f'if (Get-Process -Id {int(pid)} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}'], timeout=15)
        return result.returncode == 0
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False

def active_processes():
    return [api('/api/health')['pid'], *[j['pid'] for j in api('/api/status')['jobs'] if j['running']]]

def foreign_registration(status):
    """Changing an owned fixture into a foreign definition must block control."""
    if os.name != 'nt':
        definition = Path(status['native']['definition'])
        original = definition.read_bytes()
        changed = original + (b'<!-- foreign change -->\n' if definition.suffix == '.plist' else b'# foreign change\n')
        try:
            definition.write_bytes(changed)
            service('stop', success=False)
            assert definition.read_bytes() == changed
            assert api('/api/health') is not None
        finally:
            definition.write_bytes(original)
        target = base / 'foreign-definition'
        target.write_bytes(original)
        try:
            definition.unlink()
            definition.symlink_to(target)
            service('stop', success=False)
            assert target.read_bytes() == original
        finally:
            definition.unlink()
            definition.write_bytes(original)
    else:
        name = status['native']['task_name']
        assert re.fullmatch(r'happy-everywhere-[a-f0-9]{32}', name)
        helper = base / 'foreign-task.ps1'
        saved = base / 'original-task.xml'
        helper.write_text("""param([string]$Name,[string]$Saved,[switch]$Restore)
$ErrorActionPreference='Stop'
if ($Restore) {
  Register-ScheduledTask -TaskName $Name -TaskPath '\\' -Xml ([IO.File]::ReadAllText($Saved)) -Force | Out-Null
} else {
  $original=Export-ScheduledTask -TaskName $Name -TaskPath '\\'
  [IO.File]::WriteAllText($Saved,$original)
  [xml]$changed=$original
  $changed.Task.RegistrationInfo.Description='unrelated test registration'
  Register-ScheduledTask -TaskName $Name -TaskPath '\\' -Xml $changed.OuterXml -Force | Out-Null
}
""")
        argv = ['powershell.exe', '-NoProfile', '-NonInteractive', '-File', helper, '-Name', name, '-Saved', saved]
        try:
            run(argv)
            service('stop', success=False)
            assert api('/api/health') is not None
        finally:
            if saved.exists():
                run([*argv, '-Restore'])

def foreign_runtime(status):
    """An unchanged source file does not prove the loaded command is ours."""
    backend = status['native']['backend']
    if backend == 'task-scheduler':
        return  # task ownership is queried from the live scheduler above
    service('stop')
    definition = Path(status['native']['definition'])
    original = definition.read_bytes()
    if backend == 'launchd':
        import plistlib
        domain = f'gui/{os.getuid()}'
        target = domain + '/' + status['id']
        try:
            changed = plistlib.loads(original)
            changed['ProgramArguments'] = ['/bin/sleep', '300']
            definition.write_bytes(plistlib.dumps(changed))
            run(['/bin/launchctl', 'enable', target])
            run(['/bin/launchctl', 'bootstrap', domain, definition])
            definition.write_bytes(original)
            live = run(['/bin/launchctl', 'print', target])
            assert 'program = /bin/sleep' in live
            service('stop', success=False)
            assert 'program = /bin/sleep' in run(['/bin/launchctl', 'print', target])
        finally:
            subprocess.run(['/bin/launchctl', 'bootout', target], capture_output=True, timeout=25)
            definition.write_bytes(original)
    else:
        override = definition.with_name(definition.name + '.d')
        override.mkdir()
        change = override / '10-foreign.conf'
        try:
            change.write_text('[Service]\nExecStart=\nExecStart=/bin/sleep 300\n')
            run(['systemctl', '--user', 'daemon-reload'])
            run(['systemctl', '--user', 'start', definition.name])
            service('stop', success=False)
            assert 'path=/bin/sleep' in run(['systemctl', '--user', 'show', definition.name, '--property=ExecStart'])
            run(['systemctl', '--user', 'is-active', definition.name])
        finally:
            subprocess.run(['systemctl', '--user', 'stop', definition.name], capture_output=True, timeout=25)
            change.unlink(); override.rmdir()
            run(['systemctl', '--user', 'daemon-reload'])
    service('start')

try:
    other = base / 'b-state'
    aroot, broot = base / 'a-files', base / 'b-files'
    aroot.mkdir(); broot.mkdir()
    aid, bid = cli('init', '--state', state), cli('init', '--state', other)
    for own, peer, peer_id, root in [(state, other, bid, aroot), (other, state, aid, broot)]:
        cli('trust', '--state', own, '--cert', peer / 'identity.der')
        cli('share-init', '--state', own, '--folder', 'personal', '--root', root)
        cli('share-peer', '--state', own, '--folder', 'personal', '--peer', peer_id)
    prefix = base / "installed α $HOME %USERNAME% 'quote'"
    package = base / 'package'; package.mkdir()
    name = 'everywhere.exe' if os.name == 'nt' else 'everywhere'
    shutil.copy2(binary, package / name)
    (package / 'VERSION').write_text('service-acceptance\n')
    (package / 'SHA256SUMS').write_text(f'{sha(package / name)}  {name}\n')
    if os.name == 'nt':
        run(['pwsh', '-NoProfile', '-File', repo / 'scripts/install.ps1', '-Action', 'install', '-Package', package, '-Prefix', prefix])
    else:
        run(['sh', repo / 'scripts/install.sh', 'install', package, prefix])
    server_log = (base / 'server.stderr').open('w')
    server = subprocess.Popen([str(binary), 'sync-serve', '--state', str(other), '--folder', 'personal', '--peer', aid,
                               '--listen', '127.0.0.1:0'], stdout=subprocess.PIPE, stderr=server_log, text=True)
    # The CLI emits readiness immediately; a bounded read prevents a hung fixture.
    import concurrent.futures
    with concurrent.futures.ThreadPoolExecutor() as pool:
        try:
            line = pool.submit(server.stdout.readline).result(timeout=15)
        except BaseException:
            server.kill(); server.wait(); raise
    assert line.startswith('LISTEN '), line
    remote = line.strip().split(' ', 1)[1]
    job = {'id': 'active', 'folder': 'personal', 'peer': bid, 'address': remote, 'direction': 'connect', 'enabled': True}
    paused = dict(job, id='paused', enabled=False)
    (state / 'jobs.json').write_text(json.dumps({'active': job, 'paused': paused}))
    token = cli('management-token', '--state', state)
    port = unused_port()
    first = payload(broot / 'note', 'native service initial payload')
    # Record an attempted registration too: failed installs can leave owned native state.
    installed = True
    status = service('install', '--prefix', prefix, '--listen', f'127.0.0.1:{port}')
    assert status['healthy'] and status['native']['registered'] and status['native']['enabled'], status
    wait(lambda: sha(aroot / 'note') == first)
    assert api('/api/health')['device'] == aid
    initial_pid = api('/api/health')['pid']
    again = service('install', '--prefix', prefix, '--listen', f'127.0.0.1:{port}')
    assert again['healthy'] and api('/api/health')['pid'] == initial_pid
    foreign_registration(status)
    foreign_runtime(status)
    processes = active_processes()
    stopped = service('stop')
    assert not stopped['healthy'] and not stopped['runner_live'] and not stopped['native']['enabled'], stopped
    assert api('/api/health') is None
    assert not any(process_alive(pid) for pid in processes), 'stop returned with surviving managed processes'
    second = payload(broot / 'note', 'changed while service stopped')
    assert sha(aroot / 'note') == first
    (package / 'VERSION').write_text('service-acceptance-update\n')
    if os.name == 'nt':
        run(['pwsh', '-NoProfile', '-File', repo / 'scripts/install.ps1', '-Action', 'install', '-Package', package, '-Prefix', prefix])
    else:
        run(['sh', repo / 'scripts/install.sh', 'install', package, prefix])
    started = service('start'); assert started['healthy'], started
    wait(lambda: sha(aroot / 'note') == second)
    assert api('/api/health')['pid'] != initial_pid
    selected = json.loads((state / 'service/run.json').read_text())['selected_binary']
    assert 'service-acceptance-update-' in selected
    service('stop')
    if os.name == 'nt':
        run(['pwsh', '-NoProfile', '-File', repo / 'scripts/install.ps1', '-Action', 'rollback', '-Prefix', prefix])
    else:
        run(['sh', repo / 'scripts/install.sh', 'rollback', prefix])
    service('start')
    assert 'service-acceptance-update-' not in json.loads((state / 'service/run.json').read_text())['selected_binary']
    third = payload(aroot / 'note', 'local edit survives service restart')
    restarted = service('restart'); assert restarted['healthy'], restarted
    wait(lambda: sha(broot / 'note') == third)
    jobs = api('/api/status')['jobs']
    assert next(j for j in jobs if j['id'] == 'paused')['enabled'] is False
    # Actual manager crash: native supervision must recreate the runner/manager.
    crashed_pid = api('/api/health')['pid']
    if os.name == 'nt':
        run(['taskkill', '/PID', str(crashed_pid), '/F'])
    else:
        import signal
        os.kill(crashed_pid, signal.SIGKILL)
    wait(lambda: (api('/api/health') or {}).get('pid') not in (None, crashed_pid), seconds=130)
    fourth = payload(broot / 'note', 'native supervisor recovered the manager')
    wait(lambda: sha(aroot / 'note') == fourth)
    assert service('status')['healthy']
    processes = active_processes()
    removed = service('uninstall'); installed = False
    assert removed['installed'] is False
    assert api('/api/health') is None
    assert not any(process_alive(pid) for pid in processes), 'uninstall returned with surviving managed processes'
    backend = status['native']['backend']
    if backend == 'launchd':
        assert not Path(status['native']['definition']).exists()
        run(['/bin/launchctl', 'print', f'gui/{os.getuid()}/{status["id"]}'], success=False)
    elif backend == 'systemd':
        assert not Path(status['native']['definition']).exists()
        result = subprocess.run(['systemctl', '--user', 'show', status['id'] + '.service', '--property=LoadState'], capture_output=True, text=True, timeout=15)
        assert 'LoadState=not-found' in result.stdout, result
    else:
        name = status['native']['task_name']
        assert re.fullmatch(r'happy-everywhere-[a-f0-9]{32}', name)
        run(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', f"if (Get-ScheduledTask -TaskName '{name}' -TaskPath '\\' -ErrorAction SilentlyContinue) {{ exit 1 }} else {{ exit 0 }}"])
    assert service('status')['installed'] is False
    assert service('uninstall')['installed'] is False
    assert (state / 'identity.key.der').is_file() and (state / 'jobs.json').is_file()
    assert sha(aroot / 'note') == fourth and sha(broot / 'note') == fourth
    report = {'status': 'passed', 'platform': os.name, 'native_service': status['native']['backend'],
              'cases': ['install', 'idempotent-install', 'foreign-registration-refused', 'foreign-loaded-command-refused', 'literal-variable-paths', 'authenticated-health', 'tls-delivery', 'stop-and-child-exit', 'start', 'update', 'rollback', 'restart', 'manager-crash', 'uninstall-and-child-exit', 'idempotent-uninstall', 'preserved-paused-job', 'preserved-user-data'],
              'sha256': fourth, 'physical_reboot_verified': False, 'login_cycle_verified': False}
except BaseException as error:
    failure = error
    report = {'status': 'failed', 'error': repr(error), 'fixture': str(base)}
finally:
    if installed:
        result = subprocess.run([str(binary), 'service', 'uninstall', '--state', str(state)], capture_output=True, text=True, timeout=60)
        commands.append({'cleanup': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr})
        if result.returncode and (state / 'service/config.json').exists():
            report['cleanup_error'] = result.stderr
            if failure is None:
                failure = RuntimeError('native registration cleanup failed')
                report['status'] = 'failed'
    if server is not None:
        server.kill(); server.wait()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    args.report.with_suffix('.commands.json').write_text(json.dumps(commands, indent=2) + '\n')
    if failure is None:
        shutil.rmtree(base)
    else:
        evidence = args.report.parent / ('service-evidence-' + base.name)
        evidence.mkdir(exist_ok=False)
        for path in base.rglob('*'):
            if path.is_file() and (path.suffix in ('.json', '.log', '.sqlite', '.stderr') or path.name.endswith(('.sqlite-wal', '.sqlite-shm'))):
                target = evidence / path.relative_to(base); target.parent.mkdir(parents=True, exist_ok=True); shutil.copy2(path, target)
        print(f'Service failure fixture retained at {base}')
print(json.dumps(report, indent=2))
if failure is not None:
    raise failure
