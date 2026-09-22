# Folder synchronization quick start

This branch is an alpha build candidate. Test with disposable folders first.
Both devices need this branch's binary; the original single-file protocol is
not the folder protocol. A folder ID must match on both devices, while its
absolute path and state directory are device-local.

## Set up each device

Choose a state directory outside every synchronized folder. Create the data
folder first. Example for device A (adapt paths on Windows):

```sh
everywhere init --state /home/me/everywhere-state
everywhere share-init --state /home/me/everywhere-state --folder personal --root /home/me/Personal
```

Repeat on B with its own paths. Exchange only `identity.der`, never
`identity.key.der`. Verify the printed fingerprint over a trusted channel.
Approve each other's certificate and grant that peer access to the folder:

```sh
everywhere trust --state /home/me/everywhere-state --cert /path/to/peer-identity.der
everywhere share-peer --state /home/me/everywhere-state --folder personal --peer PEER_FINGERPRINT
```

## Connect

On B, listen on a reachable interface, permitting the chosen TCP port through
your firewall. Use B's private LAN or VPN address instead of the example:

```sh
everywhere sync-serve --state /home/me/everywhere-state --folder personal --peer A_FINGERPRINT --listen 192.168.1.20:7444
```

On A:

```sh
everywhere sync --state /home/me/everywhere-state --folder personal --peer B_FINGERPRINT --addr 192.168.1.20:7444 --continuous
```

The connection transfers changes in both directions. `--continuous` retries
and rescans; omit it for a single iteration. One folder operation holds the
folder lock, so other operations may ask you to retry. No relay or NAT
traversal is supplied. Internet reachability requires an appropriate network
or VPN configuration; a local success does not establish WAN performance.

## Review deletions, conflicts and history

A local missing file remains pending until explicitly approved. Other file
changes can continue. Stop or pause a continuous job before administrative
commands if the folder is busy:

```sh
everywhere share-scan --state STATE --folder personal
everywhere share-status --state STATE --folder personal
everywhere share-approve-deletes --state STATE --folder personal --all
everywhere share-conflicts --state STATE --folder personal
everywhere share-resolve --state STATE --folder personal --path note.txt --revision REVISION_ID
everywhere share-history --state STATE --folder personal --path note.txt
everywhere share-restore --state STATE --folder personal --path note.txt --revision REVISION_ID
```

`--all` approves every currently missing path after a fresh scan. Review the
folder before issuing it. Resolution observes the current causal heads and
publishes the chosen content as a new revision. Restoration also creates a new
revision, so the restored bytes propagate on the next synchronization.
History is retained locally; older revisions that a device never received are
not automatically transferred. Objects and displaced open files are retained,
so storage use can grow. Do not delete state or recovery directories as a
routine cleanup operation.

## Revoke and pause

```sh
everywhere share-peer --state STATE --folder personal --peer PEER_FINGERPRINT --remove
everywhere revoke --state STATE --peer PEER_FINGERPRINT
```

Removing a share grant affects that folder. Global revocation rejects the
peer's device identity. Stop the foreground process with Ctrl+C to pause.
A `.everywhere-folder` marker identifies the mounted data folder; missing or
changed markers pause work instead of interpreting an unavailable mount as
mass deletion.

## State checkpoints and index recovery

After a successful scan, use `share-backup --state STATE --folder personal
--output NEW_DIRECTORY` to retain the indexed revisions, history, tombstones,
pending deletion reviews and their content objects. `share-recover --state STATE
--folder personal --backup DIRECTORY` restores a verified checkpoint with a
fresh epoch, preserving current grants and visible files. Bidirectional and
receive-only shares then require full reconciliation with an approved exporting
peer. See [the backup/recovery procedure](state-backup.md) for runnable commands,
interrupted-recovery behavior and device-disaster-recovery limitations.

## Current filesystem limits

Symlinks, special files, non-UTF-8 names and names that cannot be safely shared
with Windows are rejected. Case/Unicode aliases stop the operation rather
than silently renaming a file. File/directory type replacement requires
manual attention. Byte contents, directories and causal revisions are synced;
ACLs, ownership, extended attributes and original timestamps are not a
portable metadata contract in this alpha. No stable NAS claim is made.
