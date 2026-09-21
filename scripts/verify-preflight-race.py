"""POSIX process-level regression for a replaced registration probe directory.

Only fresh local synthetic fixtures are used. A missed pause is retried, never
counted as a pass. Evidence is retained, including private test-only identities.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time


def exercise(binary, evidence, phase, replacement):
    for attempt in range(40):
        case = evidence / f'{phase}-{replacement}-{attempt}'
        case.mkdir()
        state, root, documents = case / 'state', case / 'root', case / 'documents'
        root.mkdir()
        documents.mkdir()
        expected = {}
        for name in ['source', 'linked', 'renamed', 'other.txt']:
            data = f'existing synthetic document: {name}\n'.encode()
            (documents / name).write_bytes(data)
            expected[name] = hashlib.sha256(data).hexdigest()
        subprocess.run([binary, 'init', '--state', str(state)], check=True,
                       capture_output=True, timeout=30)
        process = subprocess.Popen([binary, 'share-init', '--state', str(state),
                                    '--folder', 'race', '--root', str(root)],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        intercepted = False
        try:
            deadline = time.monotonic() + 10
            while process.poll() is None and time.monotonic() < deadline:
                candidates = list(root.glob('.everywhere-preflight-*'))
                if not candidates:
                    continue
                probe = candidates[0]
                if phase == 'after-source' and not (probe / 'source').exists():
                    continue
                os.kill(process.pid, signal.SIGSTOP)
                _, status = os.waitpid(process.pid, os.WUNTRACED)
                if not os.WIFSTOPPED(status):
                    process.returncode = os.waitstatus_to_exitcode(status)
                    break
                # The process may have finished preflight before STOP arrived.
                if not probe.is_dir() or (root / '.everywhere-folder').exists():
                    os.kill(process.pid, signal.SIGCONT)
                    break
                if phase == 'after-source' and not (probe / 'source').exists():
                    os.kill(process.pid, signal.SIGCONT)
                    break
                probe.rename(case / 'held-probe')
                if replacement == 'symlink':
                    # Target stays inside the share to exercise capability-confined
                    # symlink traversal, rather than an already-rejected escape.
                    documents.rename(root / 'existing-documents')
                    documents = root / 'existing-documents'
                    probe.symlink_to('existing-documents', target_is_directory=True)
                else:
                    documents.rename(probe)
                    documents = probe
                intercepted = True
                os.kill(process.pid, signal.SIGCONT)
                break
            stdout, stderr = process.communicate(timeout=30)
            (case / 'command.json').write_text(json.dumps({
                'returncode': process.returncode, 'stdout': stdout, 'stderr': stderr,
                'intercepted': intercepted,
            }, indent=2) + '\n')
            if not intercepted:
                continue
            got = {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                   for p in documents.iterdir() if p.is_file()}
            assert got == expected, f'preflight changed existing documents: {case}, {got}'
            assert process.returncode != 0, f'replaced probe was accepted: {case}'
            assert not (root / '.everywhere-folder').exists(), f'marker left: {case}'
            assert not (state / 'shares/race').exists(), f'broken registry left: {case}'
            return {'phase': phase, 'replacement': replacement, 'attempts': attempt + 1,
                    'status': 'passed', 'sha256': got}
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            process.stdout.close()
            process.stderr.close()
    raise AssertionError(f'could not intercept {phase}/{replacement}; no pass claimed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    if os.name != 'posix':
        parser.error('SIGSTOP regression requires POSIX')
    binary = str(args.binary.resolve(strict=True))
    args.report.parent.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix='preflight-race-', dir=args.report.parent)).resolve()
    report = {'status': 'running', 'evidence': str(evidence), 'cases': []}
    try:
        for phase in ['probe-created', 'after-source']:
            for replacement in ['symlink', 'directory']:
                report['cases'].append(exercise(binary, evidence, phase, replacement))
        report['status'] = 'passed'
    except BaseException as error:
        report.update(status='failed', failure=f'{type(error).__name__}: {error}')
        raise
    finally:
        args.report.write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2), flush=True)


if __name__ == '__main__':
    main()
