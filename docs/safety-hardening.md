# File safety hardening — 2026-09-22

Base: `476aca168ea6cdd2c59d64658dc86c1457d0f983` on
`codex/product-hardening`. The 53 existing macOS tests passed before changes.

## Restoring legacy archived versions

`restore` verifies the content hash encoded in an archived version's
`<path-hash>-<content-hash>` filename before opening the destination receiver.
Corrupt content, unrecognized names inside `.everywhere-versions`, and
`.corrupt-N` quarantine copies are refused without replacing the current file.
The archive parent is canonicalized, so relative paths and directory aliases
cannot bypass the archive rule. Recognized archive filenames retain their
hash check even when copied to another directory.

Restoring an ordinary user-supplied file still works. Such a file has no saved
historical digest: the operation verifies that its current bytes are copied
intact, not that they match a previously recorded version. Folder history uses
its own immutable content-object hash and revision records.

## Private historical data

On Unix, archived byte copies and newly published folder files use `0600`.
Application-owned archive, recovery, and receiver-lock directories use `0700`
at creation. Existing internal directories are tightened on write paths; a reused
archived copy is tightened as well. No automatic history deletion is performed.
These modes do not implement ownership, ACL, xattr, or executable-bit transfer.
Windows ACL hardening remains outside this change.

## Destination aliases

Receivers retain the original filename-hash lock and journal names. An
additional permanent lock entry lives at `.everywhere-locks/<filename>` in the
destination directory. The filesystem resolves filename aliases in this
namespace, avoiding incomplete Unicode case-fold approximations. Existing
destinations also use their canonical filename. Separate filenames remain
independently transferable.

All concurrently running receivers must be upgraded: older binaries do not
acquire the additional lock. Do not remove lock entries while receivers are
running. Unusual per-directory case-sensitivity configurations and Windows
short-name aliases need dedicated physical-filesystem validation. A malicious
local process replacing directory entries during path-based operations remains
outside the legacy single-file CLI's protection boundary.

## Reproduction and recovery coverage

```sh
cargo test --locked --test storage_safety -- --nocapture
cargo test --locked --test sync remote_replacement_keeps_private -- --nocapture
cargo test --locked --test sync killed_receiver -- --nocapture
cargo test --locked --all-targets
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

The storage regressions exercise actual CLI restoration, a child process with
umask `022`, relative/archive-alias paths, and filesystem-observed aliases both
before and after a destination exists. Alias cases include ASCII case,
composed/decomposed accents, `ß`/`ẞ`, and Greek combining marks. Filesystems
where a pair names distinct files must allow both receivers.

The two new restart scenarios use real mutually authenticated TCP sessions
between independent CLI processes. A SQLite trigger rejects the materialization
update **after** file publication. The test verifies new file bytes and stale DB
materialization, then forcibly kills the persistent receiver. It removes the
test trigger and reconnects with fresh processes. One scenario adopts the
published revision without generating another local edit. The other modifies
the destination while offline and requires both revisions to survive and
converge. SHA-256 independently verifies content; another exchange verifies
that causal heads stay unchanged. Failures retain the temporary device states,
objects, journals, and sender/receiver logs, printing their location.

The injected SQLite error deterministically leaves the publication/DB gap
open; it is not an actual disk-full or power-loss experiment. The subsequent
[bounded volume acceptance](volume-faults.md) separately exercises real ENOSPC,
read-only APFS media and detachment. Physical devices, NAS, WAN, power loss and
elapsed long-duration tests remain separate gates.
