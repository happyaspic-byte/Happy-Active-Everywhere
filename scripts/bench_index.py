#!/usr/bin/env python3
"""Exercise shipped folder-init/scan/index-status CLI with synthetic small files."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--files", type=int, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    if not 1 <= args.files <= 1_000_000:
        parser.error("--files must be 1..1000000")
    binary = str(args.binary.resolve())
    free = os.statvfs(args.work_dir)
    if free.f_bavail * free.f_frsize < args.files * 8192 + 2 * 1024**3:
        parser.error("insufficient capacity for synthetic small files")
    with tempfile.TemporaryDirectory(prefix="everywhere-index-", dir=args.work_dir) as temp:
        root = Path(temp)
        folder = root / "folder"
        folder.mkdir()
        db = root / "index.sqlite"
        manifest = hashlib.sha256()
        start = time.monotonic()
        for i in range(args.files):
            group = folder / f"{i // 1000:04d}"
            group.mkdir(exist_ok=True)
            name = f"{i:08d}.txt"
            data = f"synthetic index file {i}\n".encode()
            (group / name).write_bytes(data)
            manifest.update(f"{group.name}/{name}:".encode() + hashlib.sha256(data).digest())
        create_seconds = time.monotonic() - start
        common = ["--db", str(db), "--root", str(folder), "--device", "benchmark-device"]
        subprocess.run([binary, "folder-init", *common], check=True, stdout=subprocess.DEVNULL)
        start = time.monotonic()
        with (root / "events.json").open("w+") as events, (root / "scan.err").open("w+") as error:
            command = [binary, "scan", *common]
            if os.uname().sysname == "Darwin":
                command = ["/usr/bin/time", "-l", *command]
            subprocess.run(command, check=True, stdout=events, stderr=error, timeout=7200)
            initial_seconds = time.monotonic() - start
            events.seek(0)
            changes = json.load(events)
            assert len(changes) == args.files and all(e["change"] == "created" for e in changes)
            paths = {e["path"] for e in changes}
            assert len(paths) == args.files
            del changes
            error.seek(0)
            rss = next((int(line.split()[0]) for line in error if "maximum resident set size" in line), None)
        start = time.monotonic()
        rescan = subprocess.check_output([binary, "scan", *common], text=True, timeout=7200)
        rescan_seconds = time.monotonic() - start
        assert json.loads(rescan) == []
        actual = hashlib.sha256()
        for path in sorted(paths):
            actual.update(path.encode() + b":" + hashlib.sha256((folder / path).read_bytes()).digest())
        assert actual.digest() == manifest.digest(), "file contents/list changed during indexing"
        report = {
            "scope": "local index, real CLI; no file transfer",
            "files": args.files, "bytes_per_file": "variable ASCII line",
            "cache": "uncontrolled, recently created files",
            "create_seconds": create_seconds,
            "initial_scan_seconds": initial_seconds,
            "unchanged_rescan_seconds": rescan_seconds,
            "initial_scan_max_rss_bytes": rss,
            "aggregate_sha256": actual.hexdigest(),
            "missing_files": 0, "content_mismatches": 0,
            "idle_agent_rss": "not measured; scan CLI exits",
        }
        args.report.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2), flush=True)


if __name__ == "__main__":
    main()
