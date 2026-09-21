"""Measure real folder synchronization through a byte-counting TCP relay.

This is a loopback experiment, not a physical LAN/WAN measurement. Each peer
uses a separate CLI process and identity. Payload verification uses SHA-256.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import shutil
import socket
import subprocess
import tempfile
import threading
import time

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--mib', type=int, default=128)
parser.add_argument('--files', type=int, default=1000)
parser.add_argument('--report', type=Path, required=True)
parser.add_argument('--require-delta', action='store_true')
args = parser.parse_args()
assert 4 <= args.mib <= 4096 and 0 <= args.files <= 100000
binary = str(args.binary.resolve())

def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()

def run(*command):
    result = subprocess.run([binary, *map(str, command)], capture_output=True, text=True, timeout=240)
    if result.returncode:
        raise RuntimeError(result.stderr[-8000:])
    return result.stdout.strip()

class Relay:
    def __init__(self, destination):
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen(1)
        self.listener.settimeout(30)
        self.address = f'127.0.0.1:{self.listener.getsockname()[1]}'
        self.destination = destination
        self.counts = [0, 0]
        self.errors = []
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()
    def serve(self):
        try:
            with self.listener.accept()[0] as client, socket.create_connection(self.destination, timeout=30) as server:
                client.settimeout(240)
                server.settimeout(240)
                def pump(source, target, direction):
                    try:
                        while True:
                            chunk = source.recv(65536)
                            if not chunk:
                                break
                            target.sendall(chunk)
                            self.counts[direction] += len(chunk)
                    except (ConnectionResetError, BrokenPipeError):
                        pass
                    except Exception as error:
                        self.errors.append(str(error))
                    finally:
                        try:
                            target.shutdown(socket.SHUT_WR)
                        except OSError:
                            pass
                outgoing = threading.Thread(target=pump, args=(client, server, 0), daemon=True)
                incoming = threading.Thread(target=pump, args=(server, client, 1), daemon=True)
                outgoing.start(); incoming.start()
                outgoing.join(245); incoming.join(245)
                if outgoing.is_alive() or incoming.is_alive():
                    self.errors.append('relay deadline exceeded')
        except Exception as error:
            self.errors.append(str(error))
        finally:
            self.listener.close()
    def finish(self):
        self.thread.join(250)
        assert not self.thread.is_alive() and not self.errors, self.errors
        return {'client_to_server_tls_bytes': self.counts[0], 'server_to_client_tls_bytes': self.counts[1]}

with tempfile.TemporaryDirectory(prefix='everywhere-folder-bench-') as temporary:
    base = Path(temporary)
    assert shutil.disk_usage(base).free > args.mib * 1024 * 1024 * 12 + 512 * 1024 * 1024, 'insufficient benchmark disk space'
    a, b = base / 'a-state', base / 'b-state'
    ar, br = base / 'a-files', base / 'b-files'
    ar.mkdir(); br.mkdir()
    aid, bid = run('init', '--state', a), run('init', '--state', b)
    for state, root, peerstate, peer in [(a, ar, b, bid), (b, br, a, aid)]:
        run('trust', '--state', state, '--cert', peerstate / 'identity.der')
        run('share-init', '--state', state, '--folder', 'bench', '--root', root)
        run('share-peer', '--state', state, '--folder', 'bench', '--peer', peer)
    large = ar / 'large.bin'
    with large.open('wb') as stream:
        for index in range(args.mib):
            stream.write(bytes([index % 251]) * (1024 * 1024))
    (ar / 'small').mkdir()
    for index in range(args.files):
        (ar / 'small' / f'{index:08}.txt').write_bytes(f'unique synthetic file {index}\n'.encode())
    def synchronize():
        with tempfile.TemporaryFile(mode='w+b') as errors:
            server = subprocess.Popen([binary, 'sync-serve', '--state', str(b), '--folder', 'bench', '--peer', aid, '--listen', '127.0.0.1:0', '--once'], stdout=subprocess.PIPE, stderr=errors, text=True)
            try:
                ready = queue.Queue()
                threading.Thread(target=lambda: ready.put(server.stdout.readline()), daemon=True).start()
                line = ready.get(timeout=15).strip()
                assert line.startswith('LISTEN '), line
                host, port = line.removeprefix('LISTEN ').rsplit(':', 1)
                relay = Relay((host, int(port)))
                started = time.monotonic()
                run('sync', '--state', a, '--folder', 'bench', '--peer', bid, '--addr', relay.address)
                elapsed = time.monotonic() - started
                code = server.wait(timeout=30)
                errors.seek(0)
                assert code == 0, errors.read().decode(errors='replace')[-8000:]
                return {'seconds': elapsed, **relay.finish()}
            finally:
                if server.poll() is None:
                    server.kill(); server.wait()
                server.stdout.close()
    initial = synchronize()
    assert digest(large) == digest(br / 'large.bin')
    got = sorted(p.name for p in (br / 'small').iterdir())
    expected = sorted(p.name for p in (ar / 'small').iterdir())
    assert got == expected
    for name in expected:
        assert digest(ar / 'small' / name) == digest(br / 'small' / name)
    with large.open('r+b') as stream:
        stream.seek((args.mib // 2) * 1024 * 1024 + 100)
        stream.write(b'one changed range')
    delta = synchronize()
    assert digest(large) == digest(br / 'large.bin')
    unchanged = synchronize()
    rss = None
    if platform.system() == 'Linux':
        import resource
        rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss * 1024
    report = {'environment': platform.platform(), 'topology': 'two independent CLI peers on loopback through a counting TCP relay', 'large_file_mib': args.mib, 'small_files': args.files, 'initial': initial, 'single_block_edit': delta, 'unchanged': unchanged, 'max_single_child_rss_bytes': rss, 'sha256_mismatches': 0, 'missing_files': 0, 'cache_control': 'uncontrolled', 'physical_wan_verified': False}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    if args.require_delta:
        assert delta['client_to_server_tls_bytes'] < args.mib * 1024 * 1024 // 2, 'a one-block edit retransmitted most of the file'
