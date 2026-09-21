# Local management and deployment contract

The embedded management server controls the local device. It is not an
internet-exposed central multi-user service. Folder transport remains mutual
TLS between explicitly approved peers.

- `management-token --state PATH` prints a private random bearer credential
  only when explicitly requested. The HTML, URLs and ordinary startup logs
  never contain it. The browser keeps it in memory and clears it on lock.
- `manage --state PATH --listen 127.0.0.1:7445` serves bundled assets with no
  external CDNs. Non-loopback listeners are rejected. Every API request needs
  the bearer credential. Browser origins and Host headers must match the bound
  address; no permissive CORS. CSP disables inline scripts and external assets.
- Folder registration, scans, deletion review, conflict selection and history
  restoration use the same engine as the CLI. A busy folder returns a retryable
  error, never bypassing the folder lock.
- Status reads use bounded SQL summaries; they do not acquire the writer lock
  or materialize a million-entry listing. Errors are shown per folder.
- Managed jobs are explicit outgoing peer/address or incoming peer/listener
  configurations. They are visible, pausable and restartable. Their state is
  persisted independently from file content and identity keys. No automatic
  peer discovery or unapproved trust is implied.
- Installation uses a versioned binary directory and keeps the previous
  executable. Checksum verification protects artifact integrity; a checksum
  fetched from the same compromised source is not an independent signature.
- Do not downgrade across unsupported state schemas. Back up device state
  together with content/history before a destructive migration. This branch
  uses additive tables and refuses identity/epoch corruption.

Acceptance tests must exercise real HTTP authentication, origin rejection,
folder creation and state changes, as well as CLI recovery and real TLS peer
sessions. A screenshot alone is not functional verification.
