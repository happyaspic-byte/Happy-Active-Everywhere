#!/usr/bin/env python3
"""Reproducible real-CLI loopback transfer; synthetic data, independent SHA-256."""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import tempfile
import time


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--work-dir", type=pathlib.Path, required=True)
    parser.add_argument("--gib", type=int, default=1)
    parser.add_argument("--report", type=pathlib.Path, required=True)
    args = parser.parse_args()
    if args.gib < 1:
        parser.error("--gib must be positive")
    binary = str(args.binary.resolve())
    free = os.statvfs(args.work_dir)
    if free.f_bavail * free.f_frsize < (args.gib * 2 + 4) * 1024**3:
        parser.error("need source + destination + 4 GiB reserve")

    def run(*argv):
        return subprocess.check_output([binary, *map(str, argv)], text=True).strip()

    with tempfile.TemporaryDirectory(prefix="everywhere-bench-", dir=args.work_dir) as temp:
        root = pathlib.Path(temp)
        a, b = root / "a", root / "b"
        run("init", "--state", a)
        run("init", "--state", b)
        aid = run("trust", "--state", b, "--cert", a / "identity.der")
        bid = run("trust", "--state", a, "--cert", b / "identity.der")
        source, target = root / "source", root / "target"
        block = bytes((i * 31 + i // 251) % 256 for i in range(1024 * 1024))
        start = time.monotonic()
        with source.open("xb") as file:
            for _ in range(args.gib * 1024):
                file.write(block)
            file.flush()
            os.fsync(file.fileno())
        write_seconds = time.monotonic() - start
        start = time.monotonic()
        expected = sha256(source)
        hash_seconds = time.monotonic() - start
        with (root / "receiver.err").open("w+") as receiver_err, (root / "sender.err").open("w+") as sender_err:
            receiver = subprocess.Popen([binary, "receive", "--state", str(b), "--peer", aid,
                                         "--listen", "127.0.0.1:0", "--output", str(target), "--once"],
                                        stdout=subprocess.PIPE, stderr=receiver_err, text=True)
            try:
                ready = receiver.stdout.readline().strip()
                if not ready.startswith("LISTEN "):
                    raise RuntimeError(f"receiver startup failed: {ready}")
                start = time.monotonic()
                command = [binary, "send", "--state", str(a), "--peer", bid, "--addr", ready[7:], "--source", str(source)]
                if os.uname().sysname == "Darwin":
                    command = ["/usr/bin/time", "-l", *command]
                sent = subprocess.run(command, stdout=subprocess.PIPE, stderr=sender_err, text=True, timeout=3600)
                transfer_seconds = time.monotonic() - start
                received_status = receiver.wait(timeout=60)
                sender_err.seek(0)
                stderr = sender_err.read()
                receiver_err.seek(0)
                if sent.returncode or received_status:
                    raise RuntimeError(f"transfer failed: {stderr}\n{receiver_err.read()}")
                actual = sha256(target)
                assert actual == expected, "independent SHA-256 mismatch"
                rss = next((int(line.split()[0]) for line in stderr.splitlines() if "maximum resident set size" in line), None)
                report = {
                    "scope": "same-host loopback, two independent CLI processes",
                    "cache": "uncontrolled/warm source; no cache purge",
                    "gib": args.gib, "bytes": source.stat().st_size,
                    "source_write_fsync_seconds": write_seconds,
                    "source_sha256_seconds": hash_seconds,
                    "transfer_wall_seconds_including_hashes": transfer_seconds,
                    "mib_per_second": args.gib * 1024 / transfer_seconds,
                    "sender_max_rss_bytes": rss,
                    "source_sha256": expected, "destination_sha256": actual,
                    "mismatches": 0, "cli": json.loads(sent.stdout),
                    "network_ceiling": "unmeasured", "real_devices": "unverified",
                }
                args.report.write_text(json.dumps(report, indent=2) + "\n")
                print(json.dumps(report, indent=2), flush=True)
            finally:
                if receiver.poll() is None:
                    receiver.kill()
                receiver.wait()


if __name__ == "__main__":
    main()
