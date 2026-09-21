"""Negative acceptance checks for the stability test's independent oracle."""
import hashlib
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest

from soak_oracle import (Observation, assert_manifest, assert_reconciled, manifest,
                         index_snapshot, file_value)
from soak_runtime import Node, Relay, RunLock, owner_is_live, process_alive
from soak import Controller


class OracleTests(unittest.TestCase):
    def test_matching_visible_bytes_cannot_hide_equal_unresolved_conflicts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'note').write_bytes(b'expected visible bytes')
            assert_manifest(root, {'note': file_value(b'expected visible bytes')})
            snapshot = {'entries': {'note': {'heads': [
                {'clock': {'a': 1}, 'content': {'kind': 'file', 'hash': 'a' * 64}},
                {'clock': {'b': 1}, 'content': {'kind': 'deleted'}},
            ]}}, 'pending': [], 'incoming': [], 'needed': []}
            with self.assertRaises(AssertionError):
                assert_reconciled([snapshot] * 3)

    def test_equal_corruption_extra_files_and_missing_files_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            expected = {'note': {'kind': 'file', 'bytes': 8,
                                'sha256': hashlib.sha256(b'expected').hexdigest()}}
            (root / 'note').write_bytes(b'wrong!!!')
            with self.assertRaises(AssertionError):
                assert_manifest(root, expected)
            (root / 'note').write_bytes(b'expected')
            assert_manifest(root, expected)
            (root / 'unexpected').touch()
            with self.assertRaises(AssertionError):
                assert_manifest(root, expected)
            (root / 'unexpected').unlink()
            (root / 'note').unlink()
            with self.assertRaises(AssertionError):
                assert_manifest(root, expected)

    def test_empty_files_and_directories_remain_distinct(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'empty-file').touch()
            (root / 'empty-dir').mkdir()
            self.assertEqual(manifest(root), {
                'empty-file': {'kind': 'file', 'bytes': 0,
                              'sha256': 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'},
                'empty-dir': {'kind': 'directory'},
            })

    @unittest.skipUnless(os.name == 'posix', 'symlink creation permission differs on Windows')
    def test_visible_symlink_is_not_counted_as_verified_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'source').write_bytes(b'known')
            (root / 'alias').symlink_to('source')
            with self.assertRaises(AssertionError):
                manifest(root)

    def test_cycles_cannot_substitute_for_elapsed_observation(self):
        clock = Observation(100, 2, 30)
        for seconds in [0, 25, 50, 75, 99]:
            clock.observe(seconds, 1000 + seconds)
        self.assertFalse(clock.complete(99, 10000))
        clock.observe(100, 1100)
        self.assertFalse(clock.complete(100, 1))
        self.assertTrue(clock.complete(100, 2))

    def test_suspend_or_stale_observation_never_qualifies(self):
        for monotonic, utc in [(31, 1031), (1, 1031), (-1, 999)]:
            clock = Observation(1, 1, 30)
            clock.observe(0, 1000)
            with self.assertRaises(AssertionError):
                clock.observe(monotonic, utc)


class RuntimeTests(unittest.TestCase):
    def test_quiesce_refuses_new_connections_but_drains_an_existing_exchange(self):
        listener = socket.socket()
        listener.bind(('127.0.0.1', 0)); listener.listen(1); listener.settimeout(5)
        def echo():
            with listener.accept()[0] as peer:
                peer.settimeout(5)
                while data := peer.recv(4096):
                    peer.sendall(data)
        worker = threading.Thread(target=echo)
        worker.start()
        relay = Relay(listener.getsockname())
        try:
            with socket.create_connection(relay.address, timeout=5) as client:
                client.sendall(b'before')
                self.assertEqual(client.recv(4096), b'before')
                relay.quiesce()
                with socket.create_connection(relay.address, timeout=5) as refused:
                    self.assertEqual(refused.recv(1), b'')
                client.sendall(b'finish the in-flight exchange')
                self.assertEqual(client.recv(4096), b'finish the in-flight exchange')
                client.shutdown(socket.SHUT_WR)
                self.assertEqual(client.recv(1), b'')
            deadline = time.monotonic() + 5
            while relay.active_connections and time.monotonic() < deadline:
                time.sleep(.01)
            self.assertEqual(relay.active_connections, 0)
        finally:
            relay.close(); listener.close(); worker.join(timeout=5)
            self.assertFalse(worker.is_alive())

    def test_relay_stops_during_half_closed_partition_reconnect_churn(self):
        listener = socket.socket()
        listener.bind(('127.0.0.1', 0)); listener.listen(100); listener.settimeout(.1)
        stopped = threading.Event()
        peers = []
        def accept():
            while not stopped.is_set():
                try:
                    peer, _ = listener.accept()
                    peers.append(peer)  # Deliberately never send a response/EOF.
                except socket.timeout:
                    continue
                except OSError:
                    return
        worker = threading.Thread(target=accept, daemon=True)
        worker.start()
        clients = []
        try:
            for _ in range(10):
                relay = Relay(listener.getsockname())
                try:
                    for index in range(8):
                        client = socket.create_connection(relay.address, timeout=2)
                        clients.append(client)
                        client.sendall(b'forwarded')
                        if index % 2:
                            client.shutdown(socket.SHUT_WR)
                        if index % 3 == 0:
                            relay.partition(); relay.resume()
                    time.sleep(.01)
                finally:
                    started = time.monotonic()
                    relay.close()
                    self.assertLess(time.monotonic() - started, 2, 'shutdown waited for remote EOF')
        finally:
            stopped.set(); listener.close(); worker.join(timeout=2)
            for stream in clients + peers:
                stream.close()
            self.assertFalse(worker.is_alive())

    def test_real_manager_scan_and_read_only_index_snapshot(self):
        binary = Path(__file__).resolve().parents[1] / 'target/debug' / (
            'everywhere.exe' if os.name == 'nt' else 'everywhere')
        self.assertTrue(binary.is_file(), 'build the CLI before running soak acceptance')
        with tempfile.TemporaryDirectory() as temporary:
            node = Node(binary, Path(temporary), 'a')
            try:
                node.start()
                (node.root / 'note').write_bytes(b'independent fixture')
                node.api({'action': 'scan', 'folder': 'soak'})
                snapshot = index_snapshot(node.state)
                self.assertEqual(list(snapshot['entries']), ['note'])
                self.assertEqual(snapshot['pending'], [])
                self.assertEqual(snapshot['incoming'], [])
                self.assertEqual(snapshot['needed'], [])
                self.assertEqual(snapshot['sequence'], 1)
                self.assertNotIn(node.token, node.commands.read_text())
                self.assertEqual(node.health()['device'], node.id)
            finally:
                node.stop()

    def test_liveness_requires_current_lock_and_process(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'run.lock'
            owner = subprocess.Popen([
                sys.executable, '-u', '-c',
                'import sys; from pathlib import Path; from soak_runtime import RunLock; '
                'lock=RunLock(Path(sys.argv[1])); print("ready", flush=True); sys.stdin.read()',
                str(path),
            ], cwd=Path(__file__).parent, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
            try:
                self.assertEqual(owner.stdout.readline(), b'ready\n')
                self.assertTrue(owner_is_live(path, owner.pid))
                owner.kill()
                owner.wait(timeout=5)
                self.assertFalse(owner_is_live(path, owner.pid))
                # A recycled/live unrelated PID cannot turn the stale file active.
                self.assertFalse(owner_is_live(path, os.getpid()))
            finally:
                if owner.poll() is None:
                    owner.kill(); owner.wait()
                owner.stdin.close(); owner.stdout.close()

    def test_relay_counts_real_bytes_and_partition_closes_connections(self):
        listener = socket.socket()
        listener.bind(('127.0.0.1', 0)); listener.listen(1); listener.settimeout(5)
        def echo():
            with listener.accept()[0] as stream:
                while True:
                    data = stream.recv(4096)
                    if not data:
                        return
                    stream.sendall(data)
        worker = threading.Thread(target=echo)
        worker.start()
        relay = Relay(listener.getsockname())
        try:
            with socket.create_connection(relay.address, timeout=5) as client:
                client.sendall(b'actual relay payload')
                self.assertEqual(client.recv(4096), b'actual relay payload')
                deadline = time.monotonic() + 5
                while relay.total_bytes != 40 and time.monotonic() < deadline:
                    time.sleep(.01)
                self.assertEqual(relay.total_bytes, 40)
                relay.partition()
                try:
                    self.assertEqual(client.recv(4096), b'')
                except ConnectionResetError:
                    pass
        finally:
            relay.close(); listener.close(); worker.join(timeout=5)
            self.assertFalse(worker.is_alive())


class ControllerTests(unittest.TestCase):
    def setUp(self):
        self.runner = Path(__file__).with_name('soak.py').resolve()
        self.assertTrue(self.runner.is_file(), 'managed-soak runner is missing')
        self.binary = Path(__file__).resolve().parents[1] / 'target/debug' / (
            'everywhere.exe' if os.name == 'nt' else 'everywhere')

    def controller(self, root, binary=None):
        return Controller(SimpleNamespace(work_dir=root, binary=binary or self.binary,
            seconds=1, min_cycles=1, cycle_interval=0, max_bytes=4 * 1024**3,
            reserve_bytes=0))

    def test_optimization_flags_are_rejected_before_workspace_creation(self):
        with tempfile.TemporaryDirectory() as temporary:
            for flag, environment in [(['-O'], {}), ([], {'PYTHONOPTIMIZE': '1'})]:
                root = Path(temporary) / ('flag' if flag else 'environment')
                result = subprocess.run([sys.executable, *flag, self.runner, 'run',
                    '--binary', Path(temporary) / 'missing-binary', '--work-dir', root],
                    env={**os.environ, **environment}, capture_output=True, text=True, timeout=15)
                with self.subTest(flag=flag):
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('optimization', result.stderr.lower())
                    self.assertFalse(root.exists(), 'optimized run created a workspace')

    def test_retained_blobs_do_not_hide_lost_or_replaced_history(self):
        with tempfile.TemporaryDirectory() as temporary:
            controller = self.controller(Path(temporary) / 'run')
            node = Node(self.binary, controller.root, 'a')
            controller.nodes = [node]; controller.down.remove('a')
            try:
                node.start()
                for payload in [b'prior revision', b'latest visible revision']:
                    (node.root / 'note').write_bytes(payload)
                    node.api({'action': 'scan', 'folder': 'soak'})
                    controller.expected = {'note': file_value(payload)}
                    controller.converge()
                controller.verify_retained()
                path = node.state / 'shares/soak/index.sqlite'
                with sqlite3.connect(path) as database:
                    saved = database.execute('SELECT path,id,revision FROM history').fetchall()
                    database.execute('DELETE FROM history')
                self.assertEqual(node.api({'action': 'history', 'folder': 'soak', 'path': 'note'}), [])
                with self.subTest(fault='deleted'):
                    with self.assertRaises(AssertionError):
                        controller.verify_retained()
                with sqlite3.connect(path) as database:
                    database.executemany('INSERT INTO history VALUES(?,?,?)', saved)
                    first = json.loads(saved[0][2]); first['clock']['invented'] = 123
                    database.execute('UPDATE history SET revision=? WHERE path=? AND id=?',
                                     (json.dumps(first), saved[0][0], saved[0][1]))
                with self.subTest(fault='same-count-replacement'):
                    with self.assertRaises(AssertionError):
                        controller.verify_retained()
            finally:
                node.stop(); controller.event_stream.close(); controller.lock.close()

    def test_normal_overwritten_revisions_enter_independent_retention_oracle(self):
        with tempfile.TemporaryDirectory() as temporary:
            controller = self.controller(Path(temporary) / 'run')
            node = Node(self.binary, controller.root, 'a')
            controller.nodes = [node]; controller.down.remove('a')
            try:
                node.start()
                for payload in [b'original before remote overwrite', b'next revision']:
                    (node.root / 'note').write_bytes(payload)
                    node.api({'action': 'scan', 'folder': 'soak'})
                    controller.expected = {'note': file_value(payload)}
                    controller.converge()
                self.assertEqual(set(controller.retained['a'].values()), {
                    hashlib.sha256(value).hexdigest() for value in
                    [b'original before remote overwrite', b'next revision']})
            finally:
                node.stop(); controller.event_stream.close(); controller.lock.close()

    def test_cleanup_error_still_stops_other_resources_and_records_failure(self):
        class FailingClose:
            def close(self):
                raise OSError('synthetic teardown failure')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / 'run'
            controller = self.controller(root, Path(temporary) / 'missing-binary')
            node = Node(self.binary, root, 'a'); node.start()
            pid = node.process.pid
            relay = Relay(('127.0.0.1', 1))
            controller.nodes = [node]; controller.down.remove('a')
            controller.relays = [FailingClose(), relay]
            try:
                with self.assertRaises(FileNotFoundError):
                    controller.run()
                report = json.loads((root / 'summary.json').read_text())
                self.assertEqual(report['status'], 'failed')
                self.assertFalse(report['qualification_72h'])
                self.assertTrue(report['cleanup_errors'])
                self.assertFalse(process_alive(pid))
                self.assertFalse(relay.thread.is_alive())
                self.assertFalse(owner_is_live(root / 'run.lock', os.getpid()))
            finally:
                node.stop(); relay.close()
                controller.event_stream.close(); controller.lock.close()

    def test_existing_workspace_is_not_modified(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'sentinel').write_bytes(b'existing user directory')
            result = subprocess.run([sys.executable, self.runner, 'run', '--binary', self.binary,
                                     '--work-dir', root, '--seconds', '1'],
                                    capture_output=True, text=True, timeout=15)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(sorted(p.name for p in root.iterdir()), ['sentinel'])
            self.assertEqual((root / 'sentinel').read_bytes(), b'existing user directory')

    def test_stale_running_report_is_not_live_or_qualified(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'run.lock').write_bytes(b'1')
            (root / 'progress.json').write_text(json.dumps({
                'status': 'running', 'pid': os.getpid(), 'heartbeat_utc': time.time(),
                'observed_seconds': 999999, 'qualification_72h': False,
            }))
            result = subprocess.run([sys.executable, self.runner, 'status', '--work-dir', root],
                                    capture_output=True, text=True, timeout=15, check=True)
            value = json.loads(result.stdout)
            self.assertEqual(value['status'], 'interrupted')
            self.assertFalse(value['owner_live'])
            self.assertFalse(value['qualification_72h'])

    def test_controller_death_stops_its_real_managers_and_workers(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / 'new-run'
            log = (Path(temporary) / 'controller.log').open('w')
            owner = subprocess.Popen([sys.executable, self.runner, 'run', '--binary', self.binary,
                                      '--work-dir', root, '--seconds', '600', '--cycle-interval', '0'],
                                     stdout=log, stderr=log)
            pids = []
            try:
                deadline = time.monotonic() + 90
                while time.monotonic() < deadline:
                    self.assertIsNone(owner.poll(), (Path(temporary) / 'controller.log').read_text())
                    try:
                        value = json.loads((root / 'progress.json').read_text())
                        managers = [node['pid'] for node in value.get('nodes', []) if node.get('pid')]
                        workers = [job['pid'] for node in value.get('nodes', [])
                                   for job in node.get('jobs', []) if job.get('running') and job.get('pid')]
                        if len(managers) == 3 and len(workers) == 6:
                            pids = managers + workers
                            break
                    except FileNotFoundError:
                        pass
                    time.sleep(.1)
                self.assertEqual(len(pids), 9, 'managed process tree never became observable')
                owner.kill(); owner.wait(timeout=10)
                deadline = time.monotonic() + 15
                while any(process_alive(pid) for pid in pids) and time.monotonic() < deadline:
                    time.sleep(.1)
                self.assertFalse(any(process_alive(pid) for pid in pids), f'owned processes survived: {pids}')
                result = subprocess.run([sys.executable, self.runner, 'status', '--work-dir', root],
                                        capture_output=True, text=True, timeout=15, check=True)
                self.assertEqual(json.loads(result.stdout)['status'], 'interrupted')
            finally:
                if owner.poll() is None:
                    owner.kill(); owner.wait()
                log.close()


if __name__ == '__main__':
    unittest.main()
