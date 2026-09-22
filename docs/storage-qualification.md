# Filesystem registration and NAS qualification

Share registration now checks the filesystem operations required by the existing
recovery journal **before** reserving a share ID or writing a folder marker:
directory durability synchronization on Unix, durable file creation, and hard
links that refuse an existing destination. Windows retains the existing engine's
directory-sync no-op; this check does not certify Windows directory durability. The
temporary probe has a random `.everywhere-preflight-*` name; Unix probes use
0700 directories and 0600 files. Normal success and failure remove only that
probe through its original open handle, and only removes files it created.
If the probe path is replaced, checking stops and preserves the displaced probe
and replacement. Interrupted probes or failed cleanup are retained; never delete unrelated
recovery directories. Local state must remain outside the data folder.

This is a prerequisite check, not a proof of power-loss durability or NAS
compatibility. It does not weaken synchronization or overwrite protection to
accommodate a filesystem. Rejection identifies the failed operation and leaves
the requested share ID available. A pre-existing marker or a non-directory root
also fails before reserving the ID. Failures after successful preflight, such as
power loss during later registration, are not covered by this guarantee.

## Actual SMB observation, 2026-09-22

On the available Mac with its mounted SMB volume, the previous `69bf5f9` build
failed during `share-init` with OS error 45, leaving a marker and incomplete local
registry entry. A Rust backtrace identified directory `File::sync_all()`.
Independent OS probes showed ordinary `fsync()` succeeding but the directory
`F_FULLFSYNC` request and hard-link creation returning `ENOTSUP`. The installed
Rust implementation uses `F_FULLFSYNC` for `File::sync_all()` on Apple systems.

This mount cannot currently host a synchronized data folder safely under this
engine's required primitives. The fix diagnoses and refuses it without a marker
or broken registry entry. **NAS synchronization has not passed.** No actual
peer-to-peer exchange, NAS unmount, reboot, power cut, LAN/WAN throughput or
long-duration soak is implied by a passing *rejection* test. SMB server settings,
other SMB clients and other NAS filesystems may differ; qualify each environment.

## Reproduce with disposable files

```sh
cargo build --locked

# Supported local filesystem: register, scan, reject a duplicate marker, then
# register another folder and retry the rejected ID.
python3 scripts/verify-storage-preflight.py \
  --binary target/debug/everywhere --report folder-report/storage-local.json

# An already-mounted candidate that is known to lack required primitives:
python3 scripts/verify-storage-preflight.py \
  --binary target/debug/everywhere --root-parent /path/to/mounted/volume \
  --expect unsupported --report folder-report/storage-unsupported.json
```

Use `--expect supported` to require successful registration on a candidate mount.
The script creates a new `everywhere-synthetic-test-*` data directory and a local
`storage-evidence-*` directory beside the report. It never unmounts a volume or
modifies pre-existing user files. Synthetic payload bytes are checked against an
independent SHA-256 oracle. Fixtures and command results are retained for review;
their state directories contain **test-only private keys** and must not be
published. CI uploads only the summary JSON. Failure exits nonzero and still
records the error and fixture paths. Unsupported filesystems pass the rejection
test only if no folder marker or share entry is left and registration elsewhere
still works with both the same and a different ID.

The POSIX regression below pauses the actual CLI during probe creation and
after creation of its first file, replaces the probe with a symlink or another
directory containing existing synthetic documents, and resumes the CLI. All
four cases must preserve the exact file set and independent SHA-256 contents,
and refuse registration. If it cannot intercept the process, it fails instead
of reporting a pass. This test reproduced a cleanup bug in the first preflight
implementation; the final implementation keeps the original handle and tracks
which files it created. It runs on hosted Linux/macOS; no equivalent Windows
directory-replacement result is claimed.

```sh
python3 scripts/verify-preflight-race.py --binary target/debug/everywhere \
  --report folder-report/storage-race.json
```

Next qualification gates remain a supported NAS data path, three independent
peers on that path, actual separate computers and LAN/WAN, OS restart and power
loss, and elapsed 72-hour/7-day stability runs. Same-host CLI processes do not
establish those results. The 100 GiB trial remains deferred.
