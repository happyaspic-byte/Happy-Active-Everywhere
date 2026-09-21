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

Independent review and native CI corrections are recorded in
[service-review.md](service-review.md). The adapters now validate loaded commands,
serialize the complete lifecycle, preserve literal variable/quote paths, and
distinguish permission errors from missing native registrations. Windows resolves
principal names to SIDs and uses an owned supervisor plus an enabled-only scheduled
trigger to recover either manager or supervisor death.

`c734ccd` and `666f904` passed complete Ubuntu/macOS workflows. Linux native
acceptance ran 18 cases, including stopping a suspended worker. Its release
measurement used 128 MiB plus 1,000 files: 135,077,084 initial client TLS bytes,
1,060,796 after a one-block edit, and 1,466 unchanged, with zero missing files or
SHA-256 mismatches. `084886f` also passed all 17 Windows native behavior cases,
then failed fixture cleanup on an open log handle; that harness issue is corrected.
The corrected head's final all-OS workflow remains the qualification gate.

## Encrypted device recovery — 2026-09-22

Starting from all-green service milestone `bda67ab` / run `35653299417`, device
capture now encrypts identity, folder checkpoints, history and reviewed settings
with age X25519. Recovery creates a new workspace and identity by default, fresh
folder epochs, empty active grants/trust, disabled jobs and receive-only modes.
It requires full reconciliation before restoring prior modes unless the operator
explicitly accepts offline authority. Replacement of an old identity requires
its matching fingerprint and retirement assertion.

Local process tests verify SHA-256 content/history preservation, pending deletes,
new and replacement identities, key/ciphertext errors, busy/unavailable roots,
real TLS reconciliation of newer remote deletions and subsequent edits in both
directions. Forced termination before backup/recovery publication permits retry
without changing source data. Authenticated malicious paths, duplicate entries,
trailing plaintext and unsupported SQLite schemas are rejected. A regression
caught credentials being publishable inside a source share; overlapping recovery
workspaces are now refused. macOS path spelling in a test was corrected to use
canonical paths, and Clippy's needless-borrow finding was removed.

Independent review found and reproduced three defects: JSON-null materialization
for approved tombstones, permissive temporary-directory modes, and dependence
on access to old mount paths. Focused CLI regressions now pass after correction.
The first delivery run 35657255386 passed Ubuntu; Windows exposed a Unix-only
lint and the 270-file test harness deadline, and macOS saw an existing boundary
test busy-lock failure. The latter was not reproduced in 25 local runs; that
boundary test now runs in an isolated process. Exact-head three-OS delivery is
still pending. Hosted APFS acceptance also includes actual ENOSPC during device
backup and recovery; those new cases have not yet run. No local DiskImages commands
were used. Full-product physical device/NAS/WAN, reboot/power-loss and soak gates
remain open; the 100 GiB test is still deferred.

The device-recovery delivery gates above were subsequently closed by `69bf5f9`
and [workflow 35659512218](https://github.com/happyaspic-byte/Happy-Active-Everywhere/actions/runs/35659512218):
macOS/Ubuntu passed 92 Rust tests each, Windows 80, and hosted macOS passed all
seven APFS fault cases. Final packages were checked for their exact commit and
SHA-256 and now exclude private test fixtures. Those results do not close the
physical-device, NAS, WAN, reboot/power-loss or elapsed soak gates.

## Actual SMB registration failure and preflight — 2026-09-22

A bounded live trial used a new synthetic directory on an existing SMB mount,
with all device identities and SQLite state on the Mac's local filesystem.
`share-init` failed before the planned roundtrip. The old implementation left a
folder marker and an incomplete registry entry, which could also prevent later
unrelated registration. Backtrace and independent OS probes identified missing
directory `F_FULLFSYNC` and hard-link support on this mount. Ordinary `fsync`
working was insufficient for the primitives used by this engine.

Registration now preflights these primitives using a private disposable probe,
before reserving the share ID or marker. It also rejects an existing marker or
non-directory root without reserving the ID. The CLI regression failed on the
old code and passed after the change. The actual SMB rejection acceptance
verifies the diagnostic, absence of registry/marker/probe residue, independent
SHA-256 preservation and registration elsewhere using both the same and another
ID. Local supported-filesystem acceptance also passed. CI now runs that local
acceptance on all three OSes and uploads only summary JSON, excluding private
fixture state. Full delivery results for this change are recorded in the PR.

Independent review then reproduced a new cleanup race: replacing the probe with
a symlink could make cleanup delete files in the replacement. A process-level
regression reproduced the deletion before correction. Cleanup now retains the
original open handle, checks directory identity and tracks successful creates;
failed creates never authorize removing existing files. The probe does not test
ordinary replacing rename. Review also caught a Unix-only mutation causing a
Windows lint failure, and the documentation now explicitly records Windows'
existing directory-sync no-op. POSIX CI exercises the replacement regression.

[Storage qualification](storage-qualification.md) contains the repeatable
commands and boundaries. NAS synchronization is **still unsupported in this
tested configuration**. No 3-peer NAS roundtrip or long-duration process was
started after the registration failure. User files were untouched; synthetic
fixtures and private failure evidence were retained for inspection.


## Managed three-peer stability controller — local acceptance

The new [run/status controller](managed-soak.md) keeps three actual foreground
managers and six managed workers under parent-pipe ownership, exchanges real
TLS traffic through counted relays, and compares literal expected content with
independent SHA-256 and read-only causal/queue snapshots. It records copied
executable/harness hashes, elapsed continuity, sampled RSS and storage bounds.

The first local two-cycle smoke reached its scenario checks but failed relay
teardown. A rapid half-close/partition/reconnect regression reproduced a receiver
still blocked after cross-thread socket closure. A single owning thread with
cancellable nonblocking I/O now passes the same regression. Cleanup failures also
no longer mask the original failure or skip other resource cleanup and reporting.
A second regression required normal overwritten revisions, as well as conflicts,
to enter the independent retained-object oracle.

Local macOS acceptance after these changes: 14 Python guards and 93 Rust tests
passed. The two-cycle managed smoke completed 68.31 seconds of observation with
a maximum 0.70-second gap, preserved and rehashed 23 objects per device, performed
four planned manager restarts, and verified every owned process had stopped.
The unchanged interval transferred at most 14,121 TLS bytes, below the 128 KiB
bound. The run correctly reported qualification_72h=false. These numbers describe
a short same-host debug-binary run; they do not establish a 72-hour qualification.

Three-OS managed smoke Actions and a fresh independent review are required before
starting the real 72-hour run. Actual elapsed qualification, NAS/physical devices,
LAN/WAN, reboot/power loss and the deferred 100 GiB gate remain open.

The independent review found three Important false-pass paths in the controller:
equal unresolved conflicts outside prescribed scenarios, optimized Python removing
assertions, and lost/replaced history records while blobs remained intact. Each
was reproduced before correction. Normal/final reconciliation now requires one
head per path; only exact prescribed conflict revisions are exempt. Optimized
execution is rejected before fixture creation. Observed revision IDs/metadata
are remembered and rechecked through the complete history API at final acceptance.
All 17 Python checks passed after correction, including both optimization entry
paths and same-count history replacement. No Critical or Minor findings remained
from that review. Windows parent-ACL confidentiality and external qualification
remain outside the locally verified boundary.

The strengthened full smoke then caught a checkpoint race: forcibly stopping
continuous jobs could leave an already materialized revision in the incoming
queue. The retained failure had matching single heads and no needed blocks; the
queue was not ignored or cleared by the harness. Checkpoints now refuse new relay
connections, await completion of existing exchanges, then stop the jobs. A real
socket regression checks refusal of new connections while the existing exchange
can finish. Deliberate partition/crash scenarios still interrupt connections.

Final local acceptance of the complete fix pass: all 18 Python tests passed; a
fresh two-cycle smoke passed after 64.60 observed seconds, rechecked 23 content
objects and 33 exact history revisions per device, and stopped every owned
process without cleanup errors. Its unchanged interval used at most 12,694 TLS
bytes. Qualification flags remained false. The Rust source is unchanged from
the 93-test, format and Clippy run recorded above; current-head Actions will
rerun that full regression alongside the new managed smoke on every OS.

Delivery workflow `35668105650` at `5ffed9d` passed the Ubuntu job and the macOS
managed smoke, but Windows exposed two harness portability defects: readiness
stdout retained CRLF in binary mode, and SQLite context managers did not close
connections, preventing temporary database cleanup. Explicit connection closing
now covers both read-only helpers and test transactions; readiness uses universal
newline text mode. A local regression retained real SQLite connections to prove
they stayed open before correction. All 19 Python checks passed afterward.
The Windows outcome must be verified by the next exact-head Actions run.
