# Folder checkpoint and state recovery contract

This implements the existing product goal's distinction between reconnection,
index restoration and a new local scan. Device identity and current permissions
remain authoritative. A checkpoint is not a replacement for an independent
backup of user files or device credentials.

## Commands and ownership

`share-backup --state STATE --folder ID --output NEW_DIRECTORY` takes the
existing share writer lock, creates a consistent SQLite snapshot with the
SQLite backup API, and independently copies all content objects referenced by
current heads, materialization, observations and retained history. Pending
deletion reviews and tombstones are included. In-flight incoming queues are
excluded. A versioned manifest with the DB hash is written last; an incomplete
directory is not a valid checkpoint. The output must be new and outside state
and all registered data roots. Internal files/directories are private on Unix.

`share-recover --state STATE --folder ID --backup DIRECTORY` restores a
checkpoint of the same device, folder, root and marker. It requires the original
device identity and current share configuration to remain available. It does
not roll back current peer grants, global trust, direction policy or credentials.
It does not overwrite or delete visible user files. Existing content objects,
including newer objects, remain retained; corrupt cached bytes are preserved
before replacing them from verified checkpoint content.

The manifest, SQLite integrity/schema/causal records and every referenced object
are verified before the live index is displaced. A new random epoch prevents
reuse of replica counters or outgoing sequence cursors. Incoming queues and
peer cursors are cleared. A durable, validated intent covers index/config
replacement. Old SQLite files and sidecars are retained. Opening the share
finishes an interrupted intent under the same writer lock before reading state.
Share grant changes reject an unfinished intent so recovery cannot undo a
revocation made concurrently. No automatic cleanup of old state occurs.

## Reconciliation

A restored bidirectional/receive-only share is marked `recovery_pending`.
Normal local scanning, deletion approval and conflict selection wait for an
approved peer. The first network exchange requests full remote metadata and
merges it before scanning visible files. Bytes matching a known remote revision
are adopted without generating a false local edit. Unchanged stale bytes obey
a newer tombstone; distinct local edits remain concurrent and preserved.
Pending local deletion reviews remain subject to explicit approval. The hold
is cleared only after successful scan and application; failures remain retryable.

Send-only shares retain their authority model: they cannot ingest remote
metadata, so recovery rotates the epoch and resumes local reconciliation without
a receive gate. This does not grant them permission to export to an unapproved
peer. Restoring state does not change existing permission policy.

If no surviving checkpoint or peer contains a lost revision/deletion, recovery
cannot reconstruct that information. Missing original credentials/configuration
is a separate device disaster-recovery workflow; these commands must report the
boundary and never silently create an empty index under the old identity.

## Required evidence

Real CLI/TLS peers must demonstrate recovery from a corrupt/missing index,
full exchange despite old cursors, retained tombstones and pending reviews,
adoption of post-checkpoint remote bytes without new revisions, preservation of
a post-checkpoint local edit as a conflict, and stale-peer reconnection without
resurrection. SHA-256 verifies bytes independently. Corrupt/missing checkpoint
objects, malformed metadata, wrong device/root, busy shares and paths overlapping
state/data must fail safely. Restart tests cover prepared intent, displaced old
index, published new index and replaced config. Partial checkpoints and archived
raw state remain available for diagnosis.
