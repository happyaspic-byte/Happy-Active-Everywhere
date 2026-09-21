# Encrypted device backup and recovery

The product needs an operator to recover from loss of a computer, including its
identity and folder index. Existing share checkpoints require the original state
and mounted roots. This subsystem restores a complete independent workspace from
an encrypted backup. It does not declare the overall product ready for release.

## Contract

`device-keygen --output KEY` writes a new private age X25519 identity file and
prints only its public recipient. `device-backup --state STATE --recipient AGE
--output FILE` captures all registered folders, retained objects, certificate and
key, historical trust, grants and jobs. It scans before snapshotting: missing
files remain pending approval. It fails if the service, manager or a share is
busy, or a root is unavailable. It never deletes user data to make space.

`device-inspect --backup FILE --key KEY` authenticates/decrypts the entire archive
and validates every checkpoint before returning nonsecret provenance/counts.
`device-recover --backup FILE --key KEY --output NEW_WORKSPACE` does the same,
creates a new device identity, and restores `state` and `folders/<id>` in a new
workspace. Optional `--retired-device FINGERPRINT` must match the old certificate
and explicitly asserts that the original device is retired; only this option
restores its identity. Folder epochs and root markers are always new.

Restored trust and grants are empty, jobs disabled, management token and native
service registration absent. Historical settings remain in a nonsecret recovery
report for manual review. Every folder starts receive-only with reconciliation
pending, including formerly send-only folders. After re-enrollment and a full
exchange, `device-activate --state STATE --folder ID` restores its recorded mode.
`--offline-authority` explicitly accepts that newer remote changes are unknown
and allows activation without reconciliation. Neither activation grants peers
nor enables jobs. Do not use this override merely because a peer is offline.

## Representation and boundaries

Use age 0.12.1 X25519 recipient encryption, not custom cryptography. The backup
key must be retained separately from the device; losing it makes recovery
impossible. Encryption authenticates ciphertext, not the sender. Treat decrypted
data as untrusted, including SQLite schemas and all paths.

The plaintext stream is a versioned bounded JSON header followed by records
containing path length, allowed ASCII logical path, byte length, and bytes. Only
`folders/<id>/manifest.json`, `index.sqlite`, and `objects/<hash>` are allowed.
Reject duplicates, traversal, unknown records, trailing plaintext and missing
authenticated EOF. Stream large objects without compression or whole-file RAM
allocation. Use existing strict checkpoint DB/path/vector/object validation.
Limit header to 4 MiB and folders to 128. Restore bytes are a materialized indexed
snapshot, including empty directories and excluding pending local deletions;
all historical/causal objects remain available. Capture is not a filesystem-wide
transaction across unrelated external editors.

Use private temporary workspaces and files; zeroize in-memory private-key and
header buffers. Windows inherits directory ACLs, so users must select a private
local destination. No promise of secure erasure from disk/swap is made. Never
print credentials or upload backup contents as CI artifacts.

## Consistency and publication

Hold service and management lifetime locks, then a device configuration lock,
then share writer locks in sorted order throughout backup. Configuration
mutations take the shared device lock; all acquisition is nonblocking. Existing
share writer locks exclude active transfer/recovery. A busy operation fails
clearly, rather than silently omitting a folder.

Backup ciphertext is published without replacing an existing file. Recovery
builds and validates its complete tree in a private sibling staging directory,
with configs already mapped to final roots, closes DB/file handles, syncs it,
then publishes with an OS no-replace directory rename (rustix on macOS/Linux;
Windows MoveFileEx without replace). No unsupported-filesystem unsafe fallback.
An interruption before publication leaves the requested output absent; rerunning
the original command is recovery. A completed output is never overwritten.
SIGKILL may leave an owned private staging directory; report this limit and do
not automatically remove unknown leftovers. Errors/ENOSPC leave source and any
existing destination intact. No archived absolute root is ever written.

## Acceptance and remaining qualification

Actual CLI processes must prove encrypted capture, SHA-256 content recovery,
fresh and replacement identities, preserved history/pending deletion, disabled
authority/jobs, full TLS reconciliation with a newer tombstone, and subsequent
bidirectional edits after activation. Adversarial archives, wrong key, tampering,
truncation, existing destinations, busy operations and interrupted publication
must fail without replacing user files. Run all Rust checks and three OS CI.
Physical-machine migration, NAS, actual reboot, WAN and elapsed soak remain
separate product gates. The 100 GiB experiment remains deferred.

## Decision record

Atomic publication avoids an incomplete active identity and a second recovery
state machine. Fresh identity by default avoids silently cloning a live device.
Receive-only quarantine avoids the old send-only checkpoint authority exception.
The existing same-device `share-recover` semantics stay unchanged. Work proceeds
under persistent authorization to finish the product, push and test Actions;
skill workflow artifacts do not introduce another approval checkpoint.
