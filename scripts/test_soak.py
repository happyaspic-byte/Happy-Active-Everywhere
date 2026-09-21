"""Negative acceptance checks for the stability test's independent oracle."""
import hashlib
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest

from soak_oracle import Observation, assert_manifest, manifest, index_snapshot
from soak_runtime import Node, Relay, RunLock, owner_is_live


class OracleTests(unittest.TestCase):
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


if __name__ == '__main__':
    unittest.main()
