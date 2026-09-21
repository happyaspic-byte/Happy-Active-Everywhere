# Folder state recovery implementation plan

Goal: usable checkpoint and index-recovery CLI with causal safety and restart
evidence, within the authorized product-hardening work. Implement in the existing
isolated worktree; retain the Draft PR and do not publish a stable release.

Spec: [state-recovery-design.md](state-recovery-design.md).

Architecture: a `share::backup` module owns checkpoint validation, immutable
object copies and the durable index/config recovery intent. `Share::open` owns
intent resumption. The synchronization engine distinguishes restored state from
ordinary reconnection and performs remote-first reconciliation when required.

## Task 1 — checkpoint/restore boundary

- [x] Add real CLI regressions in `tests/recovery.rs`: checkpoint complete
  state, corrupt the live DB, restore and inspect bytes, causal heads, pending
  reviews, histories and current grants. Observe the missing-command failure.
- [x] Add `ShareBackup` and `ShareRecover` to `src/main.rs`; implement
  `share::backup::{create,recover}` in `src/share/backup.rs` using SQLite's
  backup feature and existing BLAKE3 object verification. Keep private modes,
  refuse overlapping paths, validate the complete checkpoint before publication.
- [x] Run `cargo test --locked --test recovery`; inspect the behavioral failures
  for the recovery handshake before moving to Task 2.

## Task 2 — reconciliation and durable restart

- [x] Add `Share::recovery_pending` and expose it through status; guard normal
  local scan/approval/selection while recovery is pending. Factor scan internals
  so the sync reconciliation can scan after merging authenticated full metadata.
- [x] Resume a validated recovery intent in `Share::open`, after taking the
  writer lock and before reading configuration/index. Preserve current grants;
  prevent concurrent grant writes while an intent is pending. Keep old DB/WAL/
  SHM/journal files in a private recovery archive; never replace an unrelated
  destination when resuming a partial operation.
- [x] Add tests for remote byte adoption, distinct local edits, post-checkpoint
  deletion, stale-peer replay, current ACL preservation, corrupt objects/index,
  absent/malformed manifest, wrong identity/root, busy locks, overlapping paths,
  and every index/config publication boundary. Expected content comes from
  literal fixtures and independent SHA-256, not production hashing helpers.
- [x] Run the recovery tests and complete Rust suite, then format, Clippy and
  release build. Obtain an independent review of causal/restart invariants.

## Task 3 — product verification and delivery

- [x] Document runnable backup/recovery commands and precise disaster-recovery
  limits in the quick start and operations documents.
- [ ] Commit the tested result, fast-forward push the work branch, and inspect
  Actions for that exact head on macOS/Windows/Ubuntu. Update the PR with actual
  results. Existing APFS fault acceptance must remain green.

Review focus: recovering from old cursors must not skip remote tombstones;
preserved post-checkpoint bytes must not become phantom edits; publication
restarts must not displace a new index twice; ACL revocation must not roll back;
checkpoint validation must not follow links or overwrite existing destinations.

Execution record: native implementation under the user's persistent autonomous
goal. No existing user state is restored during development; all destructive
fixtures are isolated temporary test data. Design/plan guidance is used without
introducing new approval checkpoints for already-authorized development.
