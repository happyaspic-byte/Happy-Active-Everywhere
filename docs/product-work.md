# Product implementation ledger

Base: `6235caf7818007fccd1eef79174d89982f9d9aa6`.
Branch: `codex/product-hardening`.

The user authorized implementation and GitHub verification of the personal
synchronization product. Completion requires observed results, not just code.

## Work order

1. Establish Ubuntu, Windows and macOS CI against the existing tests.
2. Correct data-preservation, deletion-review and long-operation behavior.
3. Connect folder metadata, vector clocks, conflict preservation and recovery
   through authenticated bidirectional transport.
4. Provide management, service installation and recoverable updates.
5. Verify complete user flows, fault handling and measured performance.

## Execution record

- Initial repository: single-file transfer alpha, 24 reported tests, no CI.
- Local checkout is a dedicated clone on the work branch. Existing main is
  unchanged. No project-specific AGENTS.md was present in the repository tree.
- Local runtime has no Rust toolchain; access to the Rust distribution server
  timed out. Run compiler and behavioral checks in GitHub Actions.
- Baseline `f53a17c`: original tests passed on Ubuntu, macOS and Windows.
  Ubuntu/macOS release builds also passed; a following push cancelled the
  Windows release build before it completed.
- Regression `bbf51da`: pending deletion blocked unrelated changes (observed
  failure). `b636ae6`: the index regression and existing index/CLI tests passed.
- Regression `b636ae6`: an open destination handle's later write was lost
  (observed failure). `ba8ae1d`: storage tests, including retained handle edits
  and interrupted-publication recovery, passed on Ubuntu.
- Regression `ba8ae1d`: a disk operation longer than peer inactivity timed out.
  `5462e1e`: progress-heartbeat tests and existing CLI transfers passed; the
  new pipelining regression failed because the old sender waited for each ACK.
- These intermediate commits deliberately contain regression failures. Only
  a subsequent all-green commit may be used as a build candidate.

## Completion boundaries

Hosted runner tests do not establish compatibility with the user's physical
devices, Synology model, storage mounts or real WAN. Long-duration tests require
their actual elapsed duration. Those gates remain unverified until executed.

## Design rulings

- Keep the current alpha classification until functional and safety gates pass.
  A build artifact is not a stable release.
- Use a dedicated clone and branch as the isolated workspace; an additional
  nested worktree would provide no protection beyond this fresh checkout.
- Preserve the displaced file itself in `.everywhere-recovery`, alongside the
  immutable historical byte copy. Publish with a no-clobber hard link. This
  closes silent replacement races without platform-specific unsafe code, but
  an existing target can briefly be absent between displacement and publication.
  A durable install intent restores it after interruption. Filesystems without
  hard-link support fail before moving an existing target. This tradeoff needs
  physical-filesystem validation before stable release.
- Protocol 2 explicitly changes framing to include progress heartbeats. Both
  ends must be upgraded; ALPN rejects protocol 1 instead of misreading frames.

## Observed cross-platform checkpoints

- `847620f`: format, Clippy, behavioral tests and release builds passed on
  Ubuntu, macOS and Windows. Transfer now pipelines up to 16 verified blocks;
  long disk work emits progress without accepting an indefinitely silent peer.
- The SQLite minimum-version regression exposed bundled SQLite 3.50.2.
  Updating rusqlite to 0.40.2 provides SQLite 3.53.2; the runtime assertion passes.
- `1c64170`: Clippy and every behavioral test passed on all three hosted OSes.
  Ubuntu ran 43 tests, including six independent-process folder scenarios:
  bidirectional Unicode/empty content, offline concurrent edits, persistent
  deletion approval, three-device convergence, concurrent deletion/edit and
  stale-peer reconnection without resurrection. Formatting failed, so the
  workflow correctly skipped release artifacts; this is not an all-green build.
- Folder metadata uses bounded pages, immutable content objects, a SQLite
  causal index, explicit share ACLs and a capability-scoped filesystem root.
  Incoming content is verified before publication. Protocol `everywhere/sync/1`
  is separate from single-file transfer `everywhere/2`.
- Conflict heads and their content are retained. Explicit conflict resolution
  and historical restore are being added with a CLI acceptance regression.

## Outstanding product gates

Management UI, managed background jobs, installation/update/rollback and
large-folder performance remain implementation work. Current folder scans
rehash content and keep the scan set in memory; the earlier single-file and
legacy-index benchmark figures do not establish folder-sync performance.
Physical device, NAS, real LAN/WAN and long-duration gates remain unexecuted.

## Management and packaging progress

- `70e8693`: the real TLS conflict-resolution and historical-restore scenario
  passed on Ubuntu. New regressions exposed a corrupt epoch panic and locally
  generated vectors exceeding accepted metadata bounds; both were corrected.
- `bd6e1f2`: token/origin checks and actual folder creation passed over HTTP.
  A regression exposed missing global revocation checks in staged application.
- `954d2f4`: revocation-before-apply and grant removal after revocation passed.
- `1a378b2`: 49 Ubuntu Rust tests passed, and Chromium exercised real login,
  registration, scan and historical restoration, verified restored disk bytes,
  checked mobile overflow and cleared the screen on lock. This intermediate
  run still failed formatting; the following push cancelled Windows's run.
- `5ae5f06`: the new managed-job scenario passed on Ubuntu, including actual
  background TLS delivery, pause, manager restart and resume. A receive-only
  object-export regression failed and was corrected in the next change.
  Browser testing caught an ambiguous duplicate folder-label locator; the
  test now scopes registration to its actual form.
- POSIX installer behavior passed locally using a disposable executable
  fixture. Cross-platform CI now tests the actual debug binary before allowing
  release packaging. Installation keeps prior binaries and checks SHA-256;
  no independent publisher signature is claimed.

Still pending: final all-green release/package run, service deployment details,
large-folder memory/scan and delta-reuse optimization, broader filesystem
fault/security coverage and the final independent branch review. Real devices,
NAS, WAN and elapsed long-duration tests remain unavailable/unexecuted.

## Complete package checkpoint

`a10c8a0` / workflow run `35631315080`: **all three OS jobs succeeded**.
Formatting, Clippy, Rust behavioral tests, actual-binary installation/update/
rollback, release compilation and platform ZIP generation all passed.
The Linux Chromium acceptance also passed. Ubuntu ran 51 Rust tests; Windows
omits Unix-only tests. Windows's earlier installer failure was the PowerShell
conversion of a null backup-path argument to an empty string; passing an
explicit NullString fixed the observed atomic-replacement regression.

This is a preserved, downloadable alpha checkpoint, not a claim of physical
NAS/WAN or long-duration qualification. Additional tests now target large
metadata pages, coexistence with the older restoration journal and measured
wire-byte reduction after editing one block of a large file.

## File-safety follow-up — 2026-09-22

Base `476aca1` was independently checked on the local Mac: 53 tests and
Clippy passed, while formatting failed. Regression tests reproduced corrupt
archive restoration, permissive archive modes, filename-alias lock bypasses,
and a private folder destination becoming readable by other users.

The fixes verify the saved archive hash before opening a receiver, reject
quarantine copies through relative or aliased parent paths, create private
Unix history/publication files, and use a filesystem-native filename lock
namespace alongside the legacy receiver locks and journals. Independent review
checked the corrections, including non-ASCII aliases missed by uppercase-only
normalization. The existing scan implementation was formatted without behavior
changes to address the latest CI format failure.

Local verification of the resulting tree:

- **64 Rust tests passed**, including 8 archive/alias tests and 15 folder tests.
- Format check and Clippy with `-D warnings` passed; release compilation passed.
- The actual release binary passed installation, upgrade, rollback, launcher,
  and corrupt-package rejection checks; macOS ARM64 ZIP packaging passed with
  Python 3.11 (the macOS system Python 3.9 lacks `tomllib`).
- An 8 MiB file plus 20 small files passed real TLS synchronization through a
  counting relay. Initial client-to-server traffic was 8,416,085 bytes, a
  one-block edit used 1,052,771 bytes, and an unchanged exchange used 1,461
  bytes. Independent SHA-256 mismatches and missing files were both zero.
- Two real-process tests inject a SQLite failure after file publication, kill
  the receiver, and reconnect. They verify exact causal-head preservation or
  preservation of a local edit made before restart. Failure evidence is kept.

## Volume-fault follow-up — 2026-09-22

Base `e3b0e2f`: all three hosted OS jobs passed workflow `35636579546`.
A new regression then reproduced a live directory-handle mismatch: replacing
the configured root with a directory containing a copied marker caused an
existing session to scan its old root and generate an incorrect tombstone.
Comparing opened directory identities fixes the observed failure. The scanner
also rechecks the root before committing its DB transaction. Local verification
passed **65 Rust tests**, formatting, Clippy and release compilation.

The [volume acceptance script](volume-faults.md) adds actual ENOSPC, detached,
read-only and in-flight-detached APFS image scenarios to macOS CI. Initial local
probes passed destination-full and state-full preservation/recovery. They also
exposed an automation issue: `hdiutil attach` can return partitions before the
whole device; detaching the first entry stalled the native utility. The harness
now explicitly selects the GPT whole device. Final qualification requires the
complete corrected script, including exact pre-recovery causal-head comparison,
to pass; earlier partial probes are not a five-scenario pass.

The corrected script at `ab1e2ce` passed all five scenarios on the hosted Mac
in workflow `35639125073`; all three OS jobs passed. Downloaded per-agent
snapshots independently matched the pre-recovery causal heads. Local DiskImages
utility operations remained stalled after the earlier partition-detach attempt;
the five-scenario qualification is hosted Mac evidence, not a local completion.

## Folder checkpoint and recovery — 2026-09-22

`share-backup` and `share-recover` add consistent SQLite/content checkpoints,
current-permission preservation, fresh epochs, retained old indexes and durable
index/configuration publication. Restored receiving shares merge full remote
metadata before scanning, so known bytes do not become invented local edits.
The procedure and its exact boundaries are in [state-backup.md](state-backup.md).

Independent review identified four gaps, each reproduced before correction:
receive-only peers releasing the recovery hold; known nonselected conflict heads
becoming phantom revisions; torn intent/configuration writes blocking recovery;
and missing schema constraints being accepted. The fixes require an exporting
peer, recognize every known pending head, atomically publish intent metadata,
regenerate configuration scratch files, and validate the complete supported
schema. Metadata length checks precede row deserialization.

Local macOS verification passed 77 Rust tests (including ten CLI recovery
scenarios and two recovery-publication unit tests), formatting, Clippy with
warnings denied, and release compilation. The CLI scenarios use independent
processes and actual TLS. Unix journal-write interruption uses a kernel file-size
limit; the six publication-boundary fixtures are deterministic restart tests,
not physical power-loss evidence. Windows database flushes use writable handles;
the exact new head still requires all three hosted OS jobs before qualification.

Initial checkpoint commit `2b28eb1` passed 77 Rust tests on Ubuntu/macOS and
68 on Windows, including all applicable recovery cases. Windows Clippy rejected
a mutable directory builder used only by the Unix permission branch. The
follow-up makes mutability platform-specific without changing directory creation.
The initial workflow is not an all-green checkpoint; the corrected head must
complete the full matrix before use as a package candidate.

Design decisions: the checkpoint covers indexed folder state and retained
objects; original credentials/current configuration are required. It preserves
current authority, including send-only policy, rather than restoring old grants.
Credentials/configuration disaster recovery, moving to another root/device,
scheduling and UI integration remain separate work. No user state was restored
during development; all mutations were isolated fixtures.

These are same-host results, not physical-device or WAN measurements.
[Safety details and commands](safety-hardening.md) describe the test boundary,
upgrade requirements, and remaining filesystem/ACL limitations. The 100 GiB
test remains deferred; no user files were removed to make space.


## Native user service — 2026-09-22

Base `da3fad8` passed all three hosted OS jobs in workflow `35643857654`:
77 Rust tests on Ubuntu/macOS and 68 on Windows, plus browser, delta, installer,
packaging and all five APFS fault cases. The preceding folder recovery delivery
is complete as a subsystem; the full product goal remains active.

The new service layer validates the installer pointer/binary, owns its manager
through a lifetime pipe, exposes authenticated process health, and preserves
paused/enabled jobs across restart. Local native launchd acceptance exercised
install/stop/start/restart, actual manager termination/recreation, retained-version
update/rollback, foreign/symlinked registration rejection, uninstall and TLS
payload hashes. Full OS login/reboot and elapsed soak remain untested.

Observed fixes: Child::wait closes stored stdin, so the bootstrap retains its
pipe independently while waiting. Current launchctl reports enabled/disabled
words rather than booleans; status now accepts both formats and rejects unknown
ones. A permission regression showed unreadable deployment state was reported as
uninstalled; metadata errors now propagate instead of being treated as absence.
The new Linux/Windows native adapters require exact-head Actions qualification;
a local Mac pass is not cross-platform proof.
