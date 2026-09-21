# Back up and recover a folder index

Pause the folder's managed jobs or stop its foreground synchronization before
administrative commands. Keep device state and the checkpoint outside every
synchronized root. Choose a new checkpoint directory for each backup:

```sh
everywhere share-scan --state STATE --folder personal
everywhere share-backup --state STATE --folder personal --output /backups/personal-2026-09-22
```

`share-scan` records current local changes and pending deletion reviews without
approving deletions. `share-backup` captures the last indexed state and all its
referenced immutable content: current revisions, historical revisions, conflicts
and tombstones. It includes pending reviews. The original device identity,
current grants, user-visible files and filesystem recovery directories remain
in their existing locations. Keep an independent protected backup of device
credentials and ordinary user data as part of device disaster recovery.

The checkpoint is private on Unix (`0700` directory, `0600` files), uses a
consistent SQLite snapshot, verifies copied content, and publishes its manifest
last. A failed/partial backup has no usable complete manifest. Existing output
directories are refused. Windows uses inherited filesystem ACLs; store backups
in an appropriately protected location. Hashes detect corruption; they do not
provide an independent signature or encryption.

## Recover a damaged or missing index

The device's original credentials and `shares/personal/config.json` must still
be available. The configured root and `.everywhere-folder` marker must match.

```sh
everywhere share-recover --state STATE --folder personal --backup /backups/personal-2026-09-22
everywhere share-status --state STATE --folder personal
```

Recovery verifies the complete checkpoint before publishing its index, generates
a fresh epoch, clears old peer cursors and retains the displaced SQLite files
under `shares/personal/index-before-<id>`. Newer cached objects are retained;
damaged cached bytes are archived before verified replacements are installed.
Current peer grants, global trust and direction policy remain in effect. The
command leaves visible user files in place.

Bidirectional and receive-only folders report `"recovery_pending": true`.
Connect to an approved peer that can export metadata and content:

```sh
everywhere sync --state STATE --folder personal --peer PEER_FINGERPRINT --addr PEER_ADDRESS:7444
```

The peer must have this build and its own approved folder/server configuration.
A receive-only remote cannot complete this reconciliation. Until an exporting
peer is available, status/history remain inspectable and ordinary scans,
deletion approval and conflict choices wait. After metadata has been merged,
the scan recognizes already known remote bytes, preserves distinct local edits,
honors newer tombstones, and keeps unapproved local deletions pending. A second
exchange may be needed to propagate local edits discovered during recovery;
continuous synchronization performs subsequent exchanges automatically.

Send-only folders keep their existing authority model: the restored index uses
a fresh epoch and permits local scanning, without ingesting remote revisions.
The command does not broaden their permissions.

If a process stops while the index/configuration replacement is in progress,
the next folder operation resumes its durable intent before opening SQLite.
Incomplete staging files are retained for diagnosis. Do not remove lock,
intent, archive or object files while an operation is running.

## Scope and observed tests

`cargo test --locked --test recovery` exercises actual CLI peers and TLS:
corrupt/missing-index recovery, pending deletion approval, post-checkpoint
tombstones, an old third peer, exact adoption of known revisions, preservation
of a distinct local edit, both direction policies, current grant revocation,
invalid checkpoints, unsupported schema constraints, symlinks and busy shares.
Payload checks use independent SHA-256. Failure fixtures retain device states,
objects and journals under `folder-report/recovery-evidence/`; the receiver log
is in each node's state directory. CI uploads failed fixtures from each OS,
excluding the disposable test devices' private identity keys.

`cargo test --locked --lib share::backup::tests` covers six durable replacement
boundaries, including a published index with both hard links still present and
a partial configuration staging file. A Unix child with an OS file-size limit
interrupts a real journal write and verifies that no incomplete final intent is
published. The boundary fixtures reopen real state; they are deterministic
interruption-state tests, not a claim of physical power-loss testing.

Information absent from every surviving checkpoint and peer cannot be recovered
from an index. Recovery after loss of the device credentials/configuration,
restoring to a different root/device, backup scheduling, encryption, and a web
backup/recovery workflow remain separate product work. Original transfer-file
journals outside the folder index are not copied by this command. Physical
devices, NAS/WAN and elapsed long-duration qualification remain unverified.
