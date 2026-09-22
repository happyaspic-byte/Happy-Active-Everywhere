"""Check share registration on a real filesystem using only new fixture folders.

State and evidence stay beside the report, outside the candidate data filesystem.
Fixtures are retained for inspection. This is a capability check, not a sync,
power-loss, physical-peer or long-duration qualification.
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--root-parent', type=Path)
    parser.add_argument('--expect', choices=['supported', 'unsupported'], default='supported')
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    binary = str(args.binary.resolve(strict=True))
    report_path = args.report.resolve()
    report_path.parent.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix='storage-evidence-', dir=report_path.parent))
    parent = args.root_parent.resolve(strict=True) if args.root_parent else evidence
    root = Path(tempfile.mkdtemp(prefix='everywhere-synthetic-test-', dir=parent))
    state = evidence / 'state'
    sentinel = b'Only this synthetic file belongs to the storage capability test.\n'
    (root / 'sentinel.txt').write_bytes(sentinel)
    expected_sha256 = hashlib.sha256(sentinel).hexdigest()
    report = {
        'environment': platform.platform(),
        'topology': 'one local CLI identity; candidate data filesystem only',
        'binary_sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
        'expected_support': args.expect,
        'evidence': str(evidence),
        'test_root': str(root),
        'sync_verified': False,
        'physical_peers_verified': False,
        'power_loss_verified': False,
        'long_duration_verified': False,
        'status': 'running',
    }
    sequence = 0
    started = time.monotonic()

    def run(*command, succeeds=True):
        nonlocal sequence
        sequence += 1
        values = [binary, *map(str, command)]
        result = subprocess.run(values, capture_output=True, text=True, timeout=90)
        (evidence / f'{sequence:03}.json').write_text(json.dumps({
            'command': values, 'returncode': result.returncode,
            'stdout': result.stdout, 'stderr': result.stderr,
        }, indent=2) + '\n')
        if succeeds and result.returncode:
            raise RuntimeError(result.stderr.strip())
        return result

    try:
        run('init', '--state', state)
        initial = run('share-init', '--state', state, '--folder', 'candidate',
                      '--root', root, succeeds=False)
        report['registration_returncode'] = initial.returncode
        report['registration_error'] = initial.stderr.strip()
        report['registration_succeeded'] = initial.returncode == 0
        if args.expect == 'unsupported':
            assert initial.returncode != 0, 'filesystem was expected to be refused'
            assert 'filesystem preflight' in initial.stderr, 'missing capability diagnosis'
            assert not (state / 'shares/candidate').exists(), 'failed preflight reserved the share ID'
            assert not (root / '.everywhere-folder').exists(), 'failed preflight left a share marker'
            assert sorted(p.name for p in root.iterdir()) == ['sentinel.txt'], 'probe residue remains'
        else:
            assert initial.returncode == 0, initial.stderr
            run('share-scan', '--state', state, '--folder', 'candidate')
            run('share-status', '--state', state, '--folder', 'candidate')
            assert sorted(p.name for p in root.iterdir()) == ['.everywhere-folder', 'sentinel.txt']
            # Existing marker failure must not poison a different local registry.
            other = evidence / 'other-state'
            run('init', '--state', other)
            rejected = run('share-init', '--state', other, '--folder', 'candidate',
                           '--root', root, succeeds=False)
            assert rejected.returncode != 0
            assert not (other / 'shares/candidate').exists(), 'existing marker reserved a broken ID'
            state = other
        # The same ID and another ID remain usable on supported local storage.
        for folder in ['unrelated', 'candidate']:
            local = evidence / f'{folder}-local'
            local.mkdir()
            run('share-init', '--state', state, '--folder', folder, '--root', local)
        report['sha256'] = hashlib.sha256((root / 'sentinel.txt').read_bytes()).hexdigest()
        assert report['sha256'] == expected_sha256, 'existing synthetic data changed'
        report['registry_reusable'] = True
        report['status'] = 'passed'
    except BaseException as error:
        report['status'] = 'failed'
        report['failure'] = f'{type(error).__name__}: {error}'
        raise
    finally:
        report['seconds'] = time.monotonic() - started
        report_path.write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2), flush=True)


if __name__ == '__main__':
    main()
