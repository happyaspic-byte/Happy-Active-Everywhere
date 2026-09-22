# Encrypted whole-device backup and recovery

This alpha CLI recovers a lost device into a **new workspace**, including folder
contents, causal versions, history, conflicts and pending deletions. Use the
existing share checkpoints when only an index was lost and the original device
state and data roots remain available.

## Capture and inspect

Stop active synchronization, the foreground manager, or the installed service
(`everywhere service stop --state STATE`). Busy state or unavailable roots makes
backup fail; no folder is silently omitted. Restart the service after backup.

```sh
everywhere device-keygen --output /private/offline/device.agekey
# Use the printed public recipient in the following command.
everywhere device-backup --state /private/device-state \
  --recipient age1YOUR_PUBLIC_RECIPIENT --output /backups/device-2026-09-22.age
everywhere device-inspect --backup /backups/device-2026-09-22.age \
  --key /private/offline/device.agekey
```

All output names must be new. The public recipient may appear in commands;
the private key is read from its file, never printed. Keep the key separately
from the computer and backup: loss of the key prevents decryption. Keep state,
keys, backups and recovery workspaces outside every synchronized folder.
Windows inherits ACLs: select a directory private to your account.

Backup scans before capturing each folder. Missing files remain pending
approval; it does not approve deletions. Current and historical objects are
included. Changing files or missing objects cause failure. This is an indexed
snapshot, not an OS-wide freeze of external editors. Unsupported links/special
files and paths remain rejected. Allow space for checkpoint copies plus the
ciphertext; inspection also needs space for decrypted checkpoints.

This captures indexed files and retained content objects. It does not clone
ignored filesystem recovery directories, unrelated files, ACLs/xattrs or native
service registrations. Keep an independent protected copy of source roots and
their recovery journals until you have checked the migration; edits retained
only in a displaced-file journal are outside this archive's indexed history.

## Restore and re-enroll

```sh
everywhere device-recover --backup /backups/device-2026-09-22.age \
  --key /private/offline/device.agekey --output /private/recovered-device
everywhere share-status --state /private/recovered-device/state --folder personal
```

Data lives in `folders/<folder-id>` beneath that workspace. Archived absolute
paths are never written. The device identity, folder epochs and root markers
are new. Peer trust and folder grants are empty, jobs disabled, native service
absent, and the management token generated afresh when management starts.
Prior settings remain in `state/device-recovery.json` and public certificates
in `state/recovery-peers`, for review only.

Every folder starts receive-only with scanning/deletion approvals held until
full reconciliation, including formerly send-only folders. Re-enroll both ends
with each current `identity.der`, `trust`, and `share-peer`. Complete a normal
`sync`/`sync-serve` exchange with a peer allowed to export metadata. This lets
newer remote tombstones remove stale restored files. Then restore the old mode:

```sh
everywhere device-activate --state /private/recovered-device/state --folder personal
```

Activation does not grant peers or enable jobs. Review addresses and peers
before enabling jobs, and install a service explicitly if needed.
`device-activate --offline-authority` explicitly treats the backup as authority
without reconciliation; it can reintroduce changes unknown to the backup. It is
not a routine workaround for an unavailable peer.

Only to replace a **retired** original device, add
`--retired-device ORIGINAL_FINGERPRINT` to recovery. The fingerprint must match
the backup. This restores the old key/certificate, while resetting epochs and
authority. The CLI cannot check whether the original computer is switched off.
Remote revocations continue to apply: restoring a key does not restore trust.

## Errors and reproducible checks

Age encryption protects confidentiality and ciphertext integrity, not sender
authenticity. Inspection/recovery authenticates the entire stream and validates
schema, paths, vectors and every referenced object. Wrong keys, truncation,
corruption and unknown/duplicate entries are rejected.

Recovery builds privately beside the target and publishes with a no-replace
directory rename. Interruption before publication leaves the requested target
absent; rerun the command. An existing result is never overwritten. SIGKILL can
leave private `.everywhere-backup-*`, `.everywhere-decrypted-*`,
`.everywhere-ciphertext-*` and `.everywhere-restore-*` artifacts. Do not register
those staging directories as live state. No automatic cleanup of unknown
leftovers or secure erasure from disk, snapshots or swap is claimed.

```sh
cargo test --locked --test device -- --nocapture
cargo test --locked --lib device::archive -- --nocapture
# Real loopback TCP/TLS, newer peer deletion, and both edit directions:
cargo test --locked --test device \
  recovery_reconciles_newer_remote_deletion_then_syncs_both_directions -- --exact
```

Tests compare independent SHA-256 values and exercise wrong keys, busy/missing
roots, private modes, preserved history/pending deletions, identity replacement,
interrupted capture/recovery and retry. Hosted macOS volume acceptance includes
device-backup-full/device-recover-full using actual APFS ENOSPC. Physical
machines, NAS/LAN/WAN, actual power loss, cross-machine migration, soak and the
deferred 100 GiB trial remain separate gates. The product remains alpha.
