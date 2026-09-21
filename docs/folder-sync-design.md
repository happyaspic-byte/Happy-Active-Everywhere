# Folder synchronization implementation contract

This extends the single-file alpha. The implementation is not a stable release
until its independent-process acceptance scenarios pass.

## State

Each share has an explicit ID, an absolute local root, a device identity, a
mount marker, peer approvals and a local direction policy. State and SQLite
remain outside the shared folder. Registering nested or overlapping roots is
rejected. Device trust alone does not grant share access.

SQLite records a set of concurrent revisions for each portable relative path.
A revision contains a vector clock and a file content hash, directory marker
or deletion tombstone. Join removes only causally dominated revisions, rejects
equal clocks with different payloads, and sorts surviving revisions by stable
ID. Receiving the same input in a different order produces the same set.
An explicit local edit/resolution observes every head and advances the local
device counter. Concurrent deletion never discards a modified file.

Content objects are retained independently from visible paths. Conflicting
heads remain retrievable; choosing a visible winner does not discard them.
Deletion approval is a local pending intent until approved. It is not exported
as a tombstone. Tombstones and conflict objects have no automatic expiry.

## Filesystem boundary

Peer paths are validated and resolved through a directory capability. Absolute
paths, parent components, reserved internal names, links, special files and
cross-platform aliases are rejected. Unicode names and empty directories are
included. A failed or incomplete scan must not become deletion evidence.

Before application, record a durable operation journal. Verify expected local
content, retain the actual displaced file and publish without overwriting a
new directory entry. Reconcile filesystem state with the journal after restart
before recording new local edits. Preserve data and report ambiguity rather
than inventing a successful operation.

## Network boundary

TLS mutual authentication and explicit share authorization precede metadata.
Exchange bounded pages with an epoch and sequence cursor. A reset epoch forces
a full reconciliation. Transfer immutable content objects through verified
blocks. Data and metadata requests must be limited to the authorized share.
Direction policy applies to metadata, content and deletion requests.

## Acceptance

Independent CLI processes must demonstrate create/edit round trips, concurrent
edits, delete/edit conflicts, approval across restart, three-peer convergence,
duplicate delivery, stale-peer reconnection and interrupted application.
Verify final file sets with independent SHA-256. Test authentication, share
denial, revoked access, marker loss, aliases and path escape. Give every wait a
deadline and retain the state needed to explain failures.

Management and installers consume this same share state; they do not implement
a separate synchronization engine. Hosted CI and physical-device verification
remain separate completion gates.
