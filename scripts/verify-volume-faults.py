"""macOS acceptance: real ENOSPC, detached and read-only synthetic volumes.

Only freshly created 256 MiB disk images are filled/detached. All CLI commands
use independent processes and real loopback TLS. Evidence is kept on success
and failure; no existing user directory or disk is modified.
"""
import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import queue
import shutil
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time

MIB = 1024 * 1024


def sha(path):
    result = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(MIB), b''):
            result.update(block)
    return result.hexdigest()


class Trial:
    def __init__(self, directory, binary):
        self.directory = directory
        self.binary = binary
        self.counter = 0
        self.mount = directory / 'mount'
        self.image = directory / 'test.dmg'
        self.device = None
        self.nodes = []
        directory.mkdir()

    def create_image(self):
        self.run(['hdiutil', 'create', '-size', '256m', '-fs', 'APFS',
                  '-volname', 'EverywhereFaultTest', '-nospotlight', str(self.image)])
        self.attach()

    def run(self, command, failure=False):
        self.counter += 1
        prefix = self.directory / f'{self.counter:03}-command'
        prefix.with_suffix('.json').write_text(json.dumps(list(map(str, command))))
        result = subprocess.run(list(map(str, command)), capture_output=True, timeout=120)
        prefix.with_suffix('.stdout').write_bytes(result.stdout)
        prefix.with_suffix('.stderr').write_bytes(result.stderr)
        if not failure and result.returncode:
            raise RuntimeError(f'{command[0:2]} failed: {result.stderr.decode(errors="replace")}')
        return result

    def attach(self, readonly=False):
        command = ['hdiutil', 'attach', str(self.image), '-nobrowse', '-noautoopen',
                   '-mountpoint', str(self.mount), '-plist']
        if readonly:
            command.append('-readonly')
        result = self.run(command)
        entities = plistlib.loads(result.stdout)['system-entities']
        # hdiutil's entity order is not stable. Detach the image's whole GPT
        # device, not whichever APFS partition happens to be listed first.
        self.device = next(e['dev-entry'] for e in entities
                           if e.get('content-hint') == 'GUID_partition_scheme')
        assert any(Path(e.get('mount-point', '/')) == self.mount for e in entities)
        assert self.mount.stat().st_dev != self.directory.stat().st_dev

    def detach(self, force=False):
        if self.device:
            self.run(['hdiutil', 'detach', self.device, *(['-force'] if force else [])])
            self.device = None

    def node(self, name, state_on_volume=False, root_on_volume=False):
        state = (self.mount if state_on_volume else self.directory) / f'{name}-state'
        root = (self.mount if root_on_volume else self.directory) / f'{name}-files'
        root.mkdir()
        identity = self.cli('init', '--state', state).stdout.decode().strip()
        node = {'name': name, 'state': state, 'root': root, 'id': identity}
        self.nodes.append(node)
        self.command(node, 'share-init', '--root', root)
        return node

    def cli(self, *args, failure=False):
        return self.run([self.binary, *args], failure=failure)

    def command(self, node, command, *args, failure=False):
        return self.cli(command, '--state', node['state'], '--folder', 'fault',
                        *args, failure=failure)

    def pair(self, state_on_volume=False):
        a = self.node('a')
        b = self.node('b', state_on_volume, not state_on_volume)
        for local, remote in [(a, b), (b, a)]:
            self.cli('trust', '--state', local['state'], '--cert', remote['state'] / 'identity.der')
            self.command(local, 'share-peer', '--peer', remote['id'])
        (a['root'] / 'note.bin').write_bytes(b'original content that must survive\n')
        self.sync(a, b)
        assert sha(a['root'] / 'note.bin') == sha(b['root'] / 'note.bin')
        return a, b

    def sync(self, a, b, failure=False, during_transfer=None):
        self.counter += 1
        errors = self.directory / f'{self.counter:03}-server.stderr'
        with errors.open('wb') as output:
            server = subprocess.Popen([self.binary, 'sync-serve', '--state', str(b['state']),
                                       '--folder', 'fault', '--peer', a['id'], '--listen',
                                       '127.0.0.1:0', '--once'], stdout=subprocess.PIPE,
                                      stderr=output, text=True)
            relay = None
            try:
                ready = queue.Queue()
                threading.Thread(target=lambda: ready.put(server.stdout.readline()), daemon=True).start()
                line = ready.get(timeout=15).strip()
                assert line.startswith('LISTEN '), line
                address = line.split(' ', 1)[1]
                if during_transfer:
                    relay = TransferBarrier(address, during_transfer)
                    address = relay.address
                result = self.command(a, 'sync', '--peer', b['id'], '--addr',
                                      address, failure=failure)
                code = server.wait(timeout=30)
                if relay:
                    relay.finish()
                if failure:
                    assert result.returncode != 0 and code != 0, 'fault was silently accepted'
                else:
                    assert code == 0, errors.read_text()
                return errors.read_text() + result.stderr.decode(errors='replace')
            finally:
                if server.poll() is None:
                    server.kill()
                    server.wait()
                server.stdout.close()
                if relay:
                    relay.close()

    def snapshot(self, node, label):
        database = node['state'] / 'shares' / 'fault' / 'index.sqlite'
        # All CLI processes have exited. Inspect a copy so Python's SQLite
        # cannot create WAL/SHM files on the volume under test.
        copy = self.directory / f'{label}-{node["name"]}.sqlite'
        shutil.copyfile(database, copy)
        wal = database.with_name(database.name + '-wal')
        if wal.exists():
            shutil.copyfile(wal, copy.with_name(copy.name + '-wal'))
        with sqlite3.connect(copy) as connection:
            rows = {table: connection.execute(f'SELECT * FROM {table} ORDER BY 1').fetchall()
                    for table in ['entries', 'pending', 'cursors']}
            (self.directory / f'{label}-{node["name"]}.sql').write_text('\n'.join(connection.iterdump()))
        (self.directory / f'{label}-{node["name"]}.json').write_text(json.dumps(rows, indent=2))
        recovery = node['root'] / '.everywhere-recovery'
        journals = []
        if recovery.is_dir():
            for path in sorted(recovery.iterdir()):
                if path.is_dir():
                    journals.append({'transaction': path.name, 'files': [
                        {'name': f.name, 'size': f.stat().st_size, 'sha256': sha(f),
                         'intent': f.read_text() if f.name == 'record.json' else None}
                        for f in sorted(path.iterdir()) if f.is_file()]})
        (self.directory / f'{label}-{node["name"]}-journals.json').write_text(json.dumps(journals, indent=2))
        return rows

    def settled(self, a, b, expected, expected_heads):
        self.sync(a, b)
        heads = []
        for node in [a, b]:
            assert sha(node['root'] / 'note.bin') == expected
            status = json.loads(self.command(node, 'share-status').stdout)
            assert status['pending_deletions'] == [], status
            assert [e['path'] for e in status['entries']] == ['note.bin'], status
            assert len(status['entries'][0]['versions']['heads']) == 1, status
            heads.append(status['entries'][0]['versions'])
            self.snapshot(node, 'recovered')
        assert heads[0] == heads[1] == expected_heads, 'recovery invented a revision or did not converge'
        for _ in range(2):
            self.sync(a, b)
            for node in [a, b]:
                status = json.loads(self.command(node, 'share-status').stdout)
                assert status['entries'][0]['versions'] == heads[0], 'rescan created extra revisions'
        # Original bytes must remain independently recoverable from retained history.
        history = json.loads(self.command(b, 'share-history', '--path', 'note.bin').stdout)
        hashes = [sha(Path(h['object_path'])) for h in history if h['object_path']]
        assert hashlib.sha256(b'original content that must survive\n').hexdigest() in hashes
        return {'final_sha256': expected, 'same_causal_heads': True, 'extra_revisions': 0,
                'pending_deletions': 0, 'original_history_preserved': True}


class TransferBarrier:
    """Pause actual encrypted object traffic after 2 MiB while detaching root."""
    def __init__(self, destination, callback):
        host, port = destination.rsplit(':', 1)
        self.destination = (host, int(port))
        self.callback = callback
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen(1)
        self.listener.settimeout(15)
        self.address = f'127.0.0.1:{self.listener.getsockname()[1]}'
        self.connections = []
        self.errors = []
        self.triggered = False
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self):
        try:
            with self.listener.accept()[0] as client, socket.create_connection(self.destination, timeout=15) as server:
                self.connections = [client, server]
                for stream in self.connections:
                    stream.settimeout(120)
                def pump(source, target, outgoing):
                    transferred = 0
                    try:
                        while True:
                            chunk = source.recv(65536)
                            if not chunk:
                                break
                            target.sendall(chunk)
                            transferred += len(chunk)
                            if outgoing and transferred >= 2 * MIB and not self.triggered:
                                self.callback(transferred)
                                self.triggered = True
                    except (ConnectionResetError, BrokenPipeError):
                        pass
                    except Exception as error:
                        self.errors.append(str(error))
                    finally:
                        try:
                            target.shutdown(socket.SHUT_WR)
                        except OSError:
                            pass
                threads = [threading.Thread(target=pump, args=(client, server, True), daemon=True),
                           threading.Thread(target=pump, args=(server, client, False), daemon=True)]
                for thread in threads:
                    thread.start()
                for thread in threads:
                    thread.join(125)
                assert not any(t.is_alive() for t in threads), 'relay deadline exceeded'
        except Exception as error:
            self.errors.append(str(error))
        finally:
            self.listener.close()

    def finish(self):
        self.thread.join(10)
        assert not self.thread.is_alive() and not self.errors and self.triggered, self.errors

    def close(self):
        for stream in self.connections:
            try:
                stream.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        self.listener.close()
        self.thread.join(5)


def fill_volume(trial):
    filler = trial.mount / '.everywhere-fault-reserve'
    chunk = os.urandom(MIB)
    observed = None
    written = 0
    with filler.open('wb', buffering=0) as stream:
        try:
            while written < 300 * MIB:
                written += stream.write(chunk)
            os.fsync(stream.fileno())
        except OSError as error:
            assert error.errno == errno.ENOSPC, error
            observed = error.errno
        assert observed == errno.ENOSPC, 'bounded image did not reach actual ENOSPC'
        # Leave room for diagnostic/transaction metadata, but not an 8 MiB object.
        stream.truncate(max(0, stream.tell() - MIB))
        os.fsync(stream.fileno())
    return filler, {'fill_errno': observed, 'bytes_written': written,
                    'free_bytes_before_sync': shutil.disk_usage(trial.mount).free}


def scenario(trial, name):
    a, b = trial.pair(state_on_volume=name == 'state-full')
    original = sha(b['root'] / 'note.bin')
    before = trial.snapshot(b, 'before-fault')
    with (a['root'] / 'note.bin').open('wb') as stream:
        for _ in range(8):
            stream.write(os.urandom(MIB))
    expected = sha(a['root'] / 'note.bin')
    trial.command(a, 'share-scan')
    expected_heads = json.loads(trial.command(a, 'share-status').stdout)['entries'][0]['versions']
    evidence = {}
    if name in ['destination-full', 'state-full']:
        filler, evidence = fill_volume(trial)
        error = trial.sync(a, b, failure=True)
        assert any(text in error.lower() for text in
                   ['space', 'full', 'os error 28', 'disk i/o error']), error
        evidence['observed_sync_error'] = error
        assert sha(b['root'] / 'note.bin') == original, 'full volume overwrote destination'
        during = trial.snapshot(b, 'during-fault')
        assert during['pending'] == before['pending'], 'full volume generated deletion approval'
        filler.unlink()  # Only this test's synthetic reserve is removed.
    elif name == 'detached':
        trial.detach()
        # An empty directory at the old mount path must not mean user deletion.
        b['root'].mkdir(parents=True)
        for command, extra in [('share-scan', []), ('share-approve-deletes', ['--all'])]:
            result = trial.command(b, command, *extra, failure=True)
            assert result.returncode != 0, 'missing volume was treated as a valid empty share'
        trial.sync(a, b, failure=True)
        assert trial.snapshot(b, 'detached') == before, 'detachment changed index/queues'
        b['root'].rmdir()
        trial.attach()
        assert sha(b['root'] / 'note.bin') == original
        evidence['empty_mount_scan_and_delete_approval_rejected'] = True
    elif name == 'read-only':
        trial.detach()
        trial.attach(readonly=True)
        error = trial.sync(a, b, failure=True)
        assert 'read-only' in error.lower() or 'os error 30' in error, error
        assert sha(b['root'] / 'note.bin') == original
        assert trial.snapshot(b, 'read-only')['pending'] == before['pending']
        trial.detach()
        trial.attach()
    elif name == 'detached-inflight':
        def detach(transferred):
            evidence['client_to_server_tls_bytes_before_detach'] = transferred
            # This is the private device created/attached by this Trial only.
            trial.detach(force=True)
        trial.sync(a, b, failure=True, during_transfer=detach)
        assert trial.snapshot(b, 'detached-inflight') == before, 'detachment changed causal index'
        trial.attach()
        assert sha(b['root'] / 'note.bin') == original
        evidence['forced_detach_during_object_transfer'] = True
    return {**evidence, **trial.settled(a, b, expected, expected_heads)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    if platform.system() != 'Darwin':
        parser.error('this acceptance requires macOS hdiutil; it is not silently skipped')
    binary = str(args.binary.resolve(strict=True))
    args.report = args.report.resolve()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    assert shutil.disk_usage(args.report.parent).free > 2 * 1024 * MIB, 'need 2 GiB free for bounded fixtures'
    base = Path(tempfile.mkdtemp(prefix='volume-evidence-', dir=args.report.parent))
    report = {'environment': platform.platform(), 'binary_sha256': sha(Path(binary)),
              'topology': 'independent CLI processes over loopback TLS', 'filesystem': 'APFS disk images',
              'image_size_mib': 256, 'evidence': str(base), 'physical_devices_verified': False,
              'power_loss_verified': False, 'scenarios': {}, 'status': 'running'}
    started = time.monotonic()
    try:
        for name in ['destination-full', 'state-full', 'detached', 'read-only', 'detached-inflight']:
            print(f'RUN {name}', flush=True)
            trial = None
            try:
                trial = Trial(base / name, binary)
                trial.create_image()
                report['scenarios'][name] = {'status': 'passed', **scenario(trial, name)}
            except Exception as error:
                report['scenarios'][name] = {'status': 'failed', 'error': str(error)}
                if trial:
                    for node in trial.nodes:
                        try:
                            trial.snapshot(node, 'failure')
                        except Exception as snapshot_error:
                            (trial.directory / f'failure-{node["name"]}.stderr').write_text(str(snapshot_error))
                raise
            finally:
                if trial:
                    trial.detach()
            print(f'PASS {name}', flush=True)
        report['status'] = 'passed'
    except Exception:
        report['status'] = 'failed'
        raise
    finally:
        report['seconds'] = time.monotonic() - started
        args.report.write_text(json.dumps(report, indent=2) + '\n')
        print(f'Evidence retained: {base}\nReport: {args.report}', flush=True)


if __name__ == '__main__':
    main()
