"""Owned process lifetimes, live locks and real TCP relays for synthetic runs."""
import json
import os
from pathlib import Path
import queue
import socket
import subprocess
import threading
import urllib.error
import urllib.request
import uuid


def atomic_json(path, value):
    path = Path(path)
    temporary = path.with_name(path.name + '.tmp-' + uuid.uuid4().hex)
    descriptor = os.open(temporary, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    with os.fdopen(descriptor, 'w', encoding='utf-8') as stream:
        json.dump(value, stream, indent=2, ensure_ascii=False)
        stream.write('\n'); stream.flush(); os.fsync(stream.fileno())
    os.replace(temporary, path)


def _lock(stream):
    stream.seek(0)
    if os.name == 'nt':
        import msvcrt
        msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
    else:
        import fcntl
        fcntl.flock(stream.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)


class RunLock:
    def __init__(self, path):
        descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
        self.stream = os.fdopen(descriptor, 'r+b')
        self.stream.write(b'1'); self.stream.flush()
        _lock(self.stream)

    def close(self):
        self.stream.close()


def process_alive(pid):
    if not isinstance(pid, int) or pid <= 0:
        return False
    if os.name == 'nt':
        # os.kill(pid, 0) is not a portable Windows liveness probe.
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        handle = kernel.OpenProcess(0x1000, False, pid)
        if not handle:
            return False
        try:
            code = wintypes.DWORD()
            return bool(kernel.GetExitCodeProcess(handle, ctypes.byref(code))) and code.value == 259
        finally:
            kernel.CloseHandle(handle)
    try:
        os.kill(pid, 0)
        return True
    except (ProcessLookupError, PermissionError):
        return False


def owner_is_live(path, pid):
    if not process_alive(pid):
        return False
    try:
        with Path(path).open('r+b') as stream:
            try:
                _lock(stream)
                return False
            except (BlockingIOError, PermissionError):
                return True
            except OSError as error:
                if error.errno in (11, 13):
                    return True
                raise
    except FileNotFoundError:
        return False


class Relay:
    def __init__(self, destination):
        self.destination = tuple(destination)
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen(8); self.listener.settimeout(.2)
        self.address = self.listener.getsockname()
        self.guard = threading.Lock()
        self.sockets = set()
        self.workers = set()
        self.counts = [0, 0]
        self.connections = 0
        self.enabled = True
        self.stopping = False
        self.thread = threading.Thread(target=self._accept, daemon=True)
        self.thread.start()

    @property
    def total_bytes(self):
        with self.guard:
            return sum(self.counts)

    @property
    def address_text(self):
        return f'{self.address[0]}:{self.address[1]}'

    def _accept(self):
        while not self.stopping:
            try:
                client, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            with self.guard:
                if not self.enabled or self.stopping:
                    client.close()
                    continue
                self.sockets.add(client)
                worker = threading.Thread(target=self._connection, args=(client,), daemon=True)
                self.workers.add(worker)
                worker.start()

    def _connection(self, client):
        server = None
        try:
            server = socket.create_connection(self.destination, timeout=3)
            client.settimeout(120); server.settimeout(120)
            client.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            server.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            with self.guard:
                if self.stopping or not self.enabled:
                    return
                self.sockets.add(server); self.connections += 1
            def pump(source, target, direction):
                try:
                    while True:
                        data = source.recv(65536)
                        if not data:
                            break
                        target.sendall(data)
                        with self.guard:
                            self.counts[direction] += len(data)
                except OSError:
                    pass  # Reset/EOF is visible to the actual TLS peers.
                finally:
                    try:
                        target.shutdown(socket.SHUT_WR)
                    except OSError:
                        pass
            outbound = threading.Thread(target=pump, args=(client, server, 0), daemon=True)
            outbound.start()
            pump(server, client, 1)
            outbound.join(timeout=121)
        except OSError:
            pass
        finally:
            client.close()
            if server is not None:
                server.close()
            with self.guard:
                self.sockets.discard(client); self.sockets.discard(server)
                self.workers.discard(threading.current_thread())

    def partition(self):
        with self.guard:
            self.enabled = False
            streams = list(self.sockets)
        for stream in streams:
            try:
                stream.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            stream.close()

    def resume(self):
        with self.guard:
            self.enabled = True

    def close(self):
        self.stopping = True
        self.partition(); self.listener.close(); self.thread.join(timeout=3)
        with self.guard:
            workers = list(self.workers)
        for worker in workers:
            worker.join(timeout=5)
        assert not self.thread.is_alive() and not any(w.is_alive() for w in workers), 'relay did not stop'


class CliError(RuntimeError):
    pass


class Node:
    def __init__(self, binary, base, name):
        assert name in ('a', 'b', 'c')
        self.binary = str(Path(binary).resolve(strict=True))
        self.base = Path(base) / name
        self.base.mkdir(mode=0o700)
        self.state, self.root = self.base / 'state', self.base / 'files'
        self.root.mkdir()
        self.commands = self.base / 'commands.jsonl'
        self.process = None
        self.address = None
        self.readers = []
        self.id = self.cli('init', '--state', self.state)
        self.cli('share-init', '--state', self.state, '--folder', 'soak', '--root', self.root)
        self.token = self.cli('management-token', '--state', self.state)
        self.name = name

    def cli(self, *arguments):
        values = list(map(str, arguments))
        result = subprocess.run([self.binary, *values], capture_output=True, text=True, timeout=90)
        secret = 'management-token' in values
        with self.commands.open('a', encoding='utf-8') as stream:
            stream.write(json.dumps({'args': values, 'returncode': result.returncode,
                                     'stdout': '[credential omitted]' if secret else result.stdout,
                                     'stderr': result.stderr}) + '\n')
        if result.returncode:
            raise CliError(result.stderr[-8000:])
        return result.stdout.strip()

    def share(self, command, *arguments):
        return json.loads(self.cli(command, '--state', self.state, '--folder', 'soak', *arguments))

    def allow(self, other):
        self.cli('trust', '--state', self.state, '--cert', other.state / 'identity.der')
        self.cli('share-peer', '--state', self.state, '--folder', 'soak', '--peer', other.id)

    def start(self):
        assert self.process is None
        self.process = subprocess.Popen([
            self.binary, 'manage', '--state', str(self.state), '--listen', '127.0.0.1:0', '--parent-watch',
        ], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        ready = queue.Queue()
        def drain(stream, name, announce):
            descriptor = os.open(self.base / name, os.O_CREAT | os.O_APPEND | os.O_WRONLY, 0o600)
            with os.fdopen(descriptor, 'a', encoding='utf-8') as log:
                first = True
                while True:
                    line = stream.readline(8192)
                    if not line:
                        break
                    log.write(line); log.flush()
                    if first and announce:
                        ready.put(line)
                    first = False
        for stream, name, announce in [(self.process.stdout, 'manager.stdout', True),
                                       (self.process.stderr, 'manager.stderr', False)]:
            reader = threading.Thread(target=drain, args=(stream, name, announce), daemon=True)
            reader.start(); self.readers.append(reader)
        try:
            line = ready.get(timeout=15).strip()
            assert line.startswith('LISTEN 127.0.0.1:'), line
            self.address = line[7:]
            self.health()
        except BaseException:
            self.stop(); raise

    def api(self, body=None, path=None):
        assert self.address is not None
        if path is None:
            path = '/api/status' if body is None else '/api/command'
        request = urllib.request.Request('http://' + self.address + path,
            data=None if body is None else json.dumps(body).encode(),
            headers={'Authorization': 'Bearer ' + self.token, 'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            raise CliError(f'HTTP {error.code}: ' + error.read(8192).decode(errors='replace')) from None

    def health(self):
        assert self.process is not None and self.process.poll() is None, f'manager exited: {self.base}'
        value = self.api(path='/api/health')
        assert value['device'] == self.id and value['pid'] == self.process.pid, 'wrong manager answered'
        return value

    def pause(self):
        for job in self.api()['jobs']:
            if job['enabled']:
                self.api({'action': 'set-job-enabled', 'id': job['id'], 'enabled': False})

    def resume(self):
        for job in self.api()['jobs']:
            if not job['enabled']:
                self.api({'action': 'set-job-enabled', 'id': job['id'], 'enabled': True})

    def stop(self):
        if self.process is None:
            return
        process, self.process = self.process, None
        if process.poll() is None:
            process.kill()
        process.wait(timeout=15)
        process.stdin.close()
        for reader in self.readers:
            reader.join(timeout=5)
        process.stdout.close(); process.stderr.close()
        self.readers.clear(); self.address = None
