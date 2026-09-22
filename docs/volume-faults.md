# Bounded filesystem fault acceptance

Run on macOS with at least 2 GiB free for synthetic fixtures:

```sh
cargo build --release --locked
python3 scripts/verify-volume-faults.py \
  --binary target/release/everywhere \
  --report folder-report/volume-local.json
```

The script creates five private **256 MiB APFS disk images**. It never fills
the host filesystem and never detaches an existing user volume. It detaches
only the whole image device returned for its own image, identified by its GPT
partition-scheme entry rather than the order of `hdiutil` results. Test images
and device states are retained, even on success, under the reported evidence
directory; no automatic history or user-file cleanup is performed.

Each scenario uses separate CLI processes, identities, state directories and
real mutually authenticated loopback TCP connections:

| Scenario | Fault and required recovery |
|---|---|
| Destination full | Write a reserved synthetic filler until actual `ENOSPC`; leave 1 MiB for metadata, then attempt an 8 MiB replacement. Preserve the old file, remove only the filler, reconnect and converge. |
| State full | Fill the volume containing the receiver's state/object database. A disk or SQLite I/O error must preserve destination bytes and pending deletions. Free only the filler, reconnect and converge. |
| Detached | Detach the receiver's data volume and put an empty directory at the old root path. Scan, deletion approval and synchronization must fail without changing entries, pending deletions or cursors. Remount the same image and converge. |
| Read-only | Remount the receiver's data image read-only. A replacement must fail without modifying the old file. Remount writable and converge. |
| Detached in flight | A TCP relay pauses the outgoing encrypted stream after at least 2 MiB, while the receiver has opened its share and is receiving content. Force-detach only this private image, resume traffic and require synchronization failure without changing the causal index. Remount and converge. |

Every successful recovery must match the source's independently computed
SHA-256 and its **exact recorded causal heads from before recovery**. Both
devices must have one expected path, one head, and no pending deletion.
Two additional exchanges must leave those heads unchanged. The original
bytes must also remain accessible through retained history and match their
independent SHA-256.

The JSON report records the executable SHA-256, environment, observed ENOSPC,
error messages, relay trigger byte count and scenario results. Per-agent SQL
dumps include entries, staged incoming metadata, needed-object queues, history
and cursors. Database copies, CLI logs and recovery-journal inventories with
SHA-256 are retained. Snapshot inspection runs on copies after CLI children
exit, so Python's SQLite cannot create WAL/SHM files on the tested volume.
GitHub's macOS job runs this acceptance against its release binary and uploads
the report, logs, database copies, SQL and journal inventories. The large disk
images remain local to the runner and are not uploaded.

## Session root identity regression

An additional Rust regression reproduced a separate bug: after opening a share,
replace its configured directory with another directory carrying a copied
`.everywhere-folder` marker. The previous implementation validated the new
marker but scanned the old open directory. Removing a file in the old directory
could therefore create a tombstone even though the new root still contained it.

Root checks now compare both open directory handles, in addition to the marker.
The scanner checks again before committing its transaction. A changed directory
aborts the session and leaves the index unchanged. A fresh session may reopen a
restored root; device identifiers are not persisted across remounts. This does
not provide an atomic OS lease against arbitrary directory replacement at every
instruction boundary.

```sh
cargo test --locked --test sync replaced_root_with_copied_marker -- --nocapture
```

## Boundaries

These are real filesystem failures on virtual APFS media and actual CLI/TLS
sessions on one host. They do not establish behavior on physical unplugging,
NAS/SMB/NFS, Windows/Linux disk-full media, storage-controller power loss, or
WAN. The in-flight test detaches during object reception, not at every file
publication/SQLite instruction boundary. Separate deterministic journal and
publication/DB-gap regressions cover selected interruption points.
The 100 GiB experiment remains deferred. Long-duration qualification requires
its actual elapsed duration and is not inferred from repeated quick runs.
