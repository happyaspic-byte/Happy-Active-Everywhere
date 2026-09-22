"""Run/status for owned, synthetic, continuously managed three-peer tests."""
import argparse
import hashlib
import json
import math
import os
import platform
from pathlib import Path
import shutil
import signal
import socket
import sqlite3
import sys
import time
import uuid

from soak_oracle import (Observation, assert_manifest, assert_reconciled, file_value,
                         history_rows, index_snapshot, manifest, sha256)
from soak_runtime import (CliError, Node, Relay, RunLock, atomic_json,
                          owner_is_live, process_alive, rss_bytes)

FOLDER = 'soak'
SENTINEL = bytes(range(256)) * 1024


class Controller:
    def __init__(self, args):
        self.args = args
        self.root = args.work_dir.resolve()
        # Refuse an existing directory before creating any credentials/processes.
        args.work_dir.mkdir(mode=0o700)
        self.root = args.work_dir.resolve(strict=True)
        self.lock = RunLock(self.root / 'run.lock')
        self.run_id = uuid.uuid4().hex
        self.nodes, self.relays = [], []
        self.expected = {}
        self.cycles = 0
        self.phase = 'setup'
        self.observation = Observation(args.seconds, args.min_cycles, 30)
        self.last_report = 0
        self.started_utc = time.time()
        self.down = set('abc')
        self.job_pids = {}
        self.job_wait = {}
        self.retained = {name: {} for name in 'abc'}
        self.revisions = {name: {} for name in 'abc'}
        self.known_revisions = {name: set() for name in 'abc'}
        self.history_cursor = {name: 0 for name in 'abc'}
        self.metrics = {'max_sampled_rss_bytes': None, 'rss_samples': 0, 'workspace_bytes': 0,
                        'max_unchanged_tls_bytes': 0, 'expected_restarts': 0}
        self.last_rss = 0
        self.latest_nodes = []
        self.cleanup_errors = []
        self.all_owned_processes_stopped = False
        self.status = 'running'
        self.binary_hash = None
        self.harness_hashes = {}
        self.binary = self.root / ('everywhere.exe' if os.name == 'nt' else 'everywhere')
        self.event_stream = (self.root / 'events.jsonl').open('x', encoding='utf-8')

    def event(self, kind, **values):
        value = {'utc': time.time(), 'cycle': self.cycles, 'phase': self.phase,
                 'event': kind, **values}
        self.event_stream.write(json.dumps(value, ensure_ascii=False) + '\n')
        self.event_stream.flush(); os.fsync(self.event_stream.fileno())

    def summary(self):
        return {
            'run_id': self.run_id, 'pid': os.getpid(), 'status': self.status,
            'started_utc': self.started_utc, 'heartbeat_utc': time.time(),
            'observed_seconds': self.observation.elapsed,
            'max_observation_gap_seconds': self.observation.max_gap,
            'target_seconds': self.args.seconds, 'cycles': self.cycles,
            'minimum_cycles': self.args.min_cycles, 'phase': self.phase,
            'binary_sha256': self.binary_hash, 'nodes': self.latest_nodes,
            'harness_sha256': self.harness_hashes,
            'python': sys.version, 'platform': platform.platform(),
            'metrics': self.metrics,
            'cleanup_errors': self.cleanup_errors,
            'all_owned_processes_stopped': self.all_owned_processes_stopped,
            'retained_objects': {name: len(values) for name, values in self.retained.items()},
            'retained_revisions': {name: len(values) for name, values in self.revisions.items()},
            'qualification_72h': (self.status == 'passed' and self.args.seconds >= 259200
                                  and self.observation.elapsed >= 259200),
            'qualification_7d': (self.status == 'passed' and self.args.seconds >= 604800
                                 and self.observation.elapsed >= 604800),
            'topology': 'three local foreground managers and six managed CLI jobs over loopback TLS relays',
            'physical_devices_verified': False, 'nas_verified': False,
            'os_reboot_verified': False, 'power_loss_verified': False,
            'fixtures_retained': True,
        }

    def pulse(self, force=False):
        now = time.monotonic()
        self.observation.observe(now, time.time())
        if not force and now - self.last_report < .5:
            return
        live = []
        for node in self.nodes:
            if node.name in self.down:
                live.append({'name': node.name, 'pid': None, 'jobs': []})
                continue
            node.health()
            jobs = node.api()['jobs']
            for job in jobs:
                key = (node.name, job['id'])
                if not job['enabled']:
                    self.job_pids.pop(key, None)
                    self.job_wait.pop(key, None)
                elif job.get('pid'):
                    previous = self.job_pids.get(key)
                    assert previous in (None, job['pid']), f'unexpected worker restart: {key}'
                    assert process_alive(job['pid']), f'unexpected worker exit: {key}'
                    self.job_pids[key] = job['pid']
                    self.job_wait.pop(key, None)
                else:
                    assert key not in self.job_pids, f'unexpected worker exit: {key}'
                    started = self.job_wait.setdefault(key, now)
                    assert now - started < 10, f'enabled worker never started: {key}'
            live.append({'name': node.name, 'pid': node.process.pid, 'jobs': jobs})
        self.latest_nodes = live
        if live and now - self.last_rss >= 15:
            pids = [pid for node in live for pid in
                    [node['pid'], *[job.get('pid') for job in node['jobs']]] if pid]
            sample = rss_bytes(pids)
            if sample is not None:
                self.metrics['max_sampled_rss_bytes'] = max(
                    self.metrics['max_sampled_rss_bytes'] or 0, sample)
                self.metrics['rss_samples'] += 1
            self.last_rss = time.monotonic()
        atomic_json(self.root / 'progress.json', self.summary())
        self.last_report = time.monotonic()

    def wait(self, predicate, description, seconds=90):
        deadline = time.monotonic() + seconds
        last = 'condition was false'
        while True:
            self.pulse()
            try:
                value = predicate()
                if value:
                    return value
            except (AssertionError, FileNotFoundError, sqlite3.OperationalError) as error:
                last = str(error)
            assert time.monotonic() < deadline, f'{description}: {last}'
            time.sleep(.2)

    def setup(self):
        self.pulse(force=True)
        source_hash = sha256(self.args.binary)
        shutil.copyfile(self.args.binary, self.binary)
        self.binary.chmod(0o500)
        self.binary_hash = sha256(self.binary)
        assert source_hash == self.binary_hash == sha256(self.args.binary), 'executable changed during snapshot'
        harness = self.root / 'harness'; harness.mkdir(mode=0o700)
        for name in ['soak.py', 'soak_runtime.py', 'soak_oracle.py', 'test_soak.py']:
            source = Path(__file__).with_name(name)
            digest = sha256(source)
            shutil.copyfile(source, harness / name)
            assert sha256(harness / name) == digest == sha256(source), 'harness changed during snapshot'
            self.harness_hashes[name] = digest
        for name in 'abc':
            node = Node(self.binary, self.root, name)
            self.nodes.append(node); node.start(); self.down.remove(name); self.pulse(force=True)
        for node in self.nodes:
            for peer in self.nodes:
                if node is not peer:
                    node.allow(peer)
            self.pulse(force=True)
        for source, target in zip(self.nodes, self.nodes[1:] + self.nodes[:1]):
            with socket.socket() as reservation:
                reservation.bind(('127.0.0.1', 0))
                port = reservation.getsockname()[1]
            relay = Relay(('127.0.0.1', port)); self.relays.append(relay)
            target.api({'action': 'save-job', 'job': {
                'id': 'listen-' + source.name, 'folder': FOLDER, 'peer': source.id,
                'address': f'127.0.0.1:{port}', 'direction': 'listen', 'enabled': False,
            }})
            source.api({'action': 'save-job', 'job': {
                'id': 'connect-' + target.name, 'folder': FOLDER, 'peer': target.id,
                'address': relay.address_text, 'direction': 'connect', 'enabled': False,
            }})
        a = self.nodes[0]
        (a.root / '문서').mkdir(); (a.root / 'empty-dir').mkdir()
        self.expected.update({'문서': {'kind': 'directory'}, 'empty-dir': {'kind': 'directory'}})
        for path, data in [('sentinel.bin', SENTINEL), ('empty.bin', b''),
                           ('문서/한글.txt', b'unchanged unicode-path fixture')]:
            self.write(a, path, data)
        self.resume()
        self.converge()
        self.checkpoint('initial')

    def write(self, node, path, data, update=True):
        self.event('write', node=node.name, path=path, expected=file_value(data))
        temporary = self.root / ('write-' + uuid.uuid4().hex)
        temporary.write_bytes(data)
        os.replace(temporary, node.root / path)
        if update:
            self.expected[path] = file_value(data)

    def pause(self, nodes=None):
        if nodes is None:
            for relay in self.relays:
                relay.quiesce()
            self.wait(lambda: not any(relay.active_connections for relay in self.relays),
                      'in-flight exchanges did not finish before checkpoint')
        for node in self.nodes if nodes is None else nodes:
            node.pause()
            self.job_pids = {key: pid for key, pid in self.job_pids.items() if key[0] != node.name}
            self.job_wait = {key: when for key, when in self.job_wait.items() if key[0] != node.name}
        self.pulse(force=True)

    def resume(self, nodes=None):
        if nodes is None:
            for relay in self.relays:
                relay.resume()
        values = self.nodes if nodes is None else nodes
        for node in values if self.cycles % 2 == 0 else list(reversed(values)):
            node.resume()
        self.pulse(force=True)

    def snapshots(self, nodes=None):
        return [index_snapshot(node.state) for node in self.nodes if nodes is None or node in nodes]

    def converge(self, nodes=None, choices=None, heads=None):
        values = self.nodes if nodes is None else nodes
        def check():
            expected = dict(self.expected)
            if choices:
                observed = manifest(values[0].root)
                for path, allowed in choices.items():
                    assert observed.get(path) in allowed, f'unknown conflict payload at {path}'
                    expected[path] = observed[path]
            for node in values:
                assert_manifest(node.root, expected)
            snapshots = self.snapshots(values)
            assert_reconciled(snapshots, expected_heads=heads)
            return expected, snapshots
        self.expected, snapshots = self.wait(check, 'peer state did not converge')
        # Capture known normal contents before the next overwrite/deletion.
        # A missing history object at final verification must fail even if all
        # peers have the correct latest visible file.
        for node, snapshot in zip(values, snapshots):
            self.capture_history(node, snapshot)
            for path, versions in snapshot['entries'].items():
                revisions = versions['heads']
                if len(revisions) == 1 and revisions[0]['content']['kind'] == 'file':
                    digest = revisions[0]['content']['hash']
                    self.remember(node, node.state / 'shares' / FOLDER / 'objects' / digest,
                                  self.expected[path]['sha256'])

    def capture_history(self, node, snapshot):
        known = self.revisions[node.name]
        # Read new rows only during the loop. At final verification, reread the
        # complete public history API to detect deletion/replacement of old rows.
        for rowid, path, identity, revision in history_rows(node.state, self.history_cursor[node.name]):
            key = (path, identity)
            assert key not in known or known[key] == revision, 'revision identity changed metadata'
            if key not in known:
                known[key] = revision
                self.known_revisions[node.name].add((path, json.dumps(revision, sort_keys=True)))
                self.event('retained-revision', node=node.name, path=path, revision_id=identity,
                           revision=revision)
            self.history_cursor[node.name] = rowid
        for path, versions in snapshot['entries'].items():
            for head in versions['heads']:
                assert (path, json.dumps(head, sort_keys=True)) in self.known_revisions[node.name], \
                    f'current revision missing from history: {node.name}/{path}'

    def checkpoint(self, phase):
        self.phase = phase
        self.pause()
        states = self.snapshots()
        assert_reconciled(states)
        for node in self.nodes:
            assert_manifest(node.root, self.expected)
        atomic_json(self.root / 'checkpoint.json', {'expected': self.expected, 'states': states})
        self.event('verified', expected=self.expected, sequences=[v['sequence'] for v in states])
        self.resume()

    def remember(self, node, object_path, digest=None):
        path = Path(object_path)
        objects = node.state / 'shares' / FOLDER / 'objects'
        assert not any(parent.is_symlink() for parent in
                       [node.base, node.state, node.state / 'shares', objects.parent, objects])
        assert path.parent.samefile(objects) and not path.is_symlink(), 'unsafe object reference'
        observed = sha256(path)
        assert digest is None or observed == digest, 'preserved object differs from expected bytes'
        digest = observed
        known = self.retained[node.name].get(path.name)
        assert known in (None, digest), 'object identity changed content'
        if known is None:
            self.retained[node.name][path.name] = digest
            self.event('retained-object', node=node.name, object=path.name, sha256=digest)
        return digest

    def conflict(self, path, payloads, deleted=0):
        self.pause()
        expected = sorted(hashlib.sha256(data).hexdigest() for data in payloads)
        chosen = None
        for node in self.nodes:
            rows = node.api({'action': 'conflicts', 'folder': FOLDER})
            assert [row['path'] for row in rows] == [path], 'unexpected or missing conflict'
            revisions = rows[0]['revisions']
            assert len(revisions) == len(payloads) + deleted
            assert sum(r['content']['kind'] == 'deleted' for r in revisions) == deleted
            got = []
            for revision in revisions:
                if revision['content']['kind'] == 'file':
                    digest = self.remember(node, revision['object_path'])
                    got.append(digest)
                    if node is self.nodes[0] and digest == expected[0]:
                        chosen = revision['id']
            assert sorted(got) == expected, 'conflict content lost or invented'
        assert chosen is not None
        self.event('resolve', path=path, chosen_sha256=expected[0])
        self.nodes[0].api({'action': 'resolve', 'folder': FOLDER, 'path': path, 'revision': chosen})
        data = next(data for data in payloads if hashlib.sha256(data).hexdigest() == expected[0])
        self.expected[path] = file_value(data)
        self.resume(); self.converge()
        assert len(self.snapshots()[0]['entries'][path]['heads']) == 1
        self.checkpoint('resolved-' + path)

    def restart(self, node):
        self.event('manager-kill', node=node.name, pid=node.process.pid)
        children = [job['pid'] for job in node.api()['jobs'] if job.get('pid')]
        self.down.add(node.name)
        node.stop()
        self.job_pids = {key: pid for key, pid in self.job_pids.items() if key[0] != node.name}
        self.job_wait = {key: when for key, when in self.job_wait.items() if key[0] != node.name}
        self.wait(lambda: not any(process_alive(pid) for pid in children), 'old workers survived manager death', 15)
        node.start(); self.down.remove(node.name)
        self.metrics['expected_restarts'] += 1
        self.pulse(force=True)

    def cycle(self):
        a, b, c = self.nodes
        tag = f'cycle {self.cycles + 1} '
        self.phase = 'roundtrip'
        self.write(a, 'roundtrip.txt', (tag + 'created A').encode()); self.converge()
        self.write(b, 'roundtrip.txt', (tag + 'edited B').encode()); self.converge()
        self.checkpoint('roundtrip')

        self.phase = 'offline-conflict'
        self.write(a, 'conflict.txt', (tag + 'common conflict basis').encode()); self.converge()
        self.pause()
        payloads, revisions = [], []
        for node in self.nodes:
            data = (tag + 'offline ' + node.name).encode(); payloads.append(data)
            self.write(node, 'conflict.txt', data, update=False)
            node.api({'action': 'scan', 'folder': FOLDER})
            revisions.extend(index_snapshot(node.state)['entries']['conflict.txt']['heads'])
        self.resume()
        self.converge(choices={'conflict.txt': [file_value(data) for data in payloads]},
                      heads={'conflict.txt': revisions})
        self.conflict('conflict.txt', payloads)

        self.phase = 'delete-versus-edit'
        self.write(a, 'delete-edit.txt', (tag + 'delete basis').encode()); self.converge()
        self.pause()
        self.event('remove', node='a', path='delete-edit.txt')
        (a.root / 'delete-edit.txt').unlink()
        a.api({'action': 'scan', 'folder': FOLDER})
        review = a.api({'action': 'pending', 'folder': FOLDER})
        assert len(review) == 1 and review[0]['path'] == 'delete-edit.txt'
        a.api({'action': 'approve-deletion', 'folder': FOLDER, 'path': 'delete-edit.txt',
               'expected': review[0]['versions']})
        data = (tag + 'offline B edit survives').encode()
        self.write(b, 'delete-edit.txt', data); b.api({'action': 'scan', 'folder': FOLDER})
        revisions = [*index_snapshot(a.state)['entries']['delete-edit.txt']['heads'],
                     *index_snapshot(b.state)['entries']['delete-edit.txt']['heads']]
        self.resume(); self.converge(heads={'delete-edit.txt': revisions})
        self.conflict('delete-edit.txt', [data], deleted=1)

        self.phase = 'pending-restart-and-stale-peer'
        old = (tag + 'C must not resurrect this').encode()
        self.write(a, 'approved.txt', old); self.converge()
        self.pause([c]); self.relays[1].partition(); self.relays[2].partition()
        self.pause([a])
        self.event('remove', node='a', path='approved.txt'); (a.root / 'approved.txt').unlink()
        a.api({'action': 'scan', 'folder': FOLDER})
        review = a.api({'action': 'pending', 'folder': FOLDER})
        assert len(review) == 1 and review[0]['path'] == 'approved.txt'
        self.restart(a)
        assert a.api({'action': 'pending', 'folder': FOLDER}) == review
        command = {'action': 'approve-deletion', 'folder': FOLDER, 'path': 'approved.txt',
                   'expected': review[0]['versions']}
        self.event('approve-deletion', node='a', path='approved.txt')
        a.api(command); approved = index_snapshot(a.state)
        try:
            a.api(command)
            raise AssertionError('replayed deletion approval was accepted')
        except CliError as error:
            assert 'HTTP 409' in str(error) and 'stale' in str(error), str(error)
        assert index_snapshot(a.state) == approved, 'duplicate approval changed state'
        self.expected.pop('approved.txt')
        self.resume([a]); self.converge(nodes=[a, b])
        assert (c.root / 'approved.txt').read_bytes() == old
        self.relays[1].resume(); self.relays[2].resume(); self.resume([c])
        self.converge(heads={'approved.txt': approved['entries']['approved.txt']['heads']})
        self.checkpoint('stale-peer-reconciled')

        self.phase = 'manager-restart'
        self.restart(b)
        self.write(c, 'roundtrip.txt', (tag + 'after manager restart C').encode()); self.converge()
        self.checkpoint('manager-restart')

        self.phase = 'unchanged-rescans'
        self.pause()
        before = self.snapshots()
        for _ in range(3):
            for node in self.nodes:
                node.api({'action': 'scan', 'folder': FOLDER})
        assert self.snapshots() == before, 'remote changes were reissued by scanning'
        transferred = sum(relay.total_bytes for relay in self.relays)
        self.resume()
        until = time.monotonic() + 3
        while time.monotonic() < until:
            self.pulse(); time.sleep(.2)
        self.pause()
        assert self.snapshots() == before, 'unchanged sync produced versions or queued work'
        transferred = sum(relay.total_bytes for relay in self.relays) - transferred
        assert transferred < len(SENTINEL) // 2, f'unchanged transfer loop: {transferred} TLS bytes'
        self.metrics['max_unchanged_tls_bytes'] = max(self.metrics['max_unchanged_tls_bytes'], transferred)
        self.event('unchanged-verified', tls_bytes=transferred)
        self.resume(); self.converge()
        self.cycles += 1
        self.checkpoint('cycle-complete')

    def resources(self):
        total = 0
        for directory, _, files in os.walk(self.root):
            for name in files:
                total += (Path(directory) / name).stat().st_size
            self.pulse()
        self.metrics['workspace_bytes'] = total
        assert total <= self.args.max_bytes, 'workspace safety bound reached; evidence retained'
        assert shutil.disk_usage(self.root).free >= self.args.reserve_bytes, 'free-space reserve reached'
        self.verify_provenance()

    def verify_provenance(self):
        assert sha256(self.binary) == self.binary_hash, 'snapshotted executable changed'
        for name, digest in self.harness_hashes.items():
            assert sha256(self.root / 'harness' / name) == digest, 'snapshotted harness changed'

    def verify_retained(self):
        assert_reconciled(self.snapshots())
        for node in self.nodes:
            assert_manifest(node.root, self.expected)
            expected = self.revisions[node.name]
            actual = {}
            for path in sorted({path for path, _ in expected}):
                rows = node.api({'action': 'history', 'folder': FOLDER, 'path': path})
                for row in rows:
                    key = (path, row['id'])
                    assert key not in actual, 'duplicate history identity'
                    actual[key] = {'clock': row['clock'], 'content': row['content']}
                self.pulse()
            assert actual == expected, f'historical revisions lost or changed: {node.name}'
            for object_name, digest in self.retained[node.name].items():
                self.remember(node, node.state / 'shares' / FOLDER / 'objects' / object_name, digest)
                self.pulse()

    def run(self):
        failure = None
        try:
            self.setup()
            while not self.observation.complete(time.monotonic(), self.cycles):
                cycle_start = time.monotonic()
                self.cycle(); self.resources()
                self.event('cycle-complete', elapsed_seconds=time.monotonic() - cycle_start)
                while time.monotonic() - cycle_start < self.args.cycle_interval:
                    self.pulse()
                    if self.observation.complete(time.monotonic(), self.cycles):
                        break
                    time.sleep(.5)
            self.phase = 'final-verification'
            self.verify_provenance()
            self.pause(); self.verify_retained()
            self.pulse(force=True)
            assert self.observation.complete(time.monotonic(), self.cycles)
            self.status = 'verified-awaiting-cleanup'
        except BaseException as error:
            failure = error
            self.status = 'interrupted' if isinstance(error, (KeyboardInterrupt, SystemExit)) else 'failed'
            self.event('failure', error=f'{type(error).__name__}: {error}')
            for node in self.nodes:
                try:
                    atomic_json(node.base / 'failure-index.json', index_snapshot(node.state))
                except Exception as snapshot_error:
                    self.event('snapshot-failed', node=node.name, error=str(snapshot_error))
        finally:
            self.cleanup()
        if failure is not None:
            raise failure
        if self.cleanup_errors:
            raise RuntimeError('owned process/network cleanup failed; see summary.json')

    def cleanup(self):
        pids = {value for node in self.latest_nodes for value in
                [node.get('pid'), *[job.get('pid') for job in node.get('jobs', [])]] if value}
        for node in self.nodes:
            if node.process is not None:
                pids.add(node.process.pid)
                if node.process.poll() is None:
                    try:
                        pids.update(job['pid'] for job in node.api()['jobs'] if job.get('pid'))
                    except Exception as error:
                        self.cleanup_errors.append(f'{node.name} final job query: {error}')
            try:
                node.stop()
            except Exception as error:
                self.cleanup_errors.append(f'{node.name} stop: {error}')
        for index, relay in enumerate(self.relays):
            try:
                relay.close()
            except Exception as error:
                self.cleanup_errors.append(f'relay {index}: {error}')
        deadline = time.monotonic() + 15
        while any(process_alive(pid) for pid in pids) and time.monotonic() < deadline:
            time.sleep(.1)
        survivors = sorted(pid for pid in pids if process_alive(pid))
        self.all_owned_processes_stopped = not survivors
        if survivors:
            self.cleanup_errors.append(f'owned processes still present: {survivors}')
        if self.cleanup_errors:
            self.status = 'failed'
        elif self.status == 'verified-awaiting-cleanup':
            self.status = 'passed'
        # Preserve the final observed job configuration separately from liveness.
        for node in self.latest_nodes:
            node['observed_pid'], node['pid'] = node.get('pid'), None
            node['stopped'] = self.all_owned_processes_stopped
            for job in node['jobs']:
                job['observed_pid'], job['pid'] = job.get('pid'), None
                job['running'] = False
        try:
            self.event('terminal', status=self.status, cleanup_errors=self.cleanup_errors)
            value = self.summary()
            atomic_json(self.root / 'summary.json', value)
            atomic_json(self.root / 'progress.json', value)
            print(json.dumps(value, indent=2), flush=True)
        finally:
            self.event_stream.close(); self.lock.close()


def status(path):
    value = json.loads((path / 'progress.json').read_text())
    live = owner_is_live(path / 'run.lock', value.get('pid'))
    value['owner_live'] = live
    if value['status'] in ('running', 'verified-awaiting-cleanup'):
        if not live:
            value['status'] = 'interrupted'
        elif abs(time.time() - value.get('heartbeat_utc', 0)) > 30:
            value['status'] = 'unresponsive'
        value['qualification_72h'] = False
        value['qualification_7d'] = False
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    run = commands.add_parser('run')
    run.add_argument('--binary', type=Path, required=True)
    run.add_argument('--work-dir', type=Path, required=True)
    run.add_argument('--seconds', type=float, default=120)
    run.add_argument('--min-cycles', type=int, default=2)
    run.add_argument('--cycle-interval', type=float, default=60)
    run.add_argument('--max-bytes', type=int, default=4 * 1024**3)
    run.add_argument('--reserve-bytes', type=int, default=2 * 1024**3)
    inspect = commands.add_parser('status')
    inspect.add_argument('--work-dir', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'status':
        print(json.dumps(status(args.work_dir), indent=2))
        return
    assert math.isfinite(args.seconds) and args.seconds > 0 and args.min_cycles > 0
    assert math.isfinite(args.cycle_interval) and args.cycle_interval >= 0
    assert args.max_bytes > 0 and args.reserve_bytes >= 0
    assert shutil.disk_usage(args.work_dir.parent).free >= args.reserve_bytes, 'insufficient free space'
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt('controller termination requested')
    signal.signal(signal.SIGTERM, interrupted)
    Controller(args).run()


if __name__ == '__main__':
    main()
