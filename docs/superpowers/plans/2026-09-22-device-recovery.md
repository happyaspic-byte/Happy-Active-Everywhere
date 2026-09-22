# Device Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans
> inline, with one fresh independent review after implementation.

**Goal:** Recover device identity, folder bytes and history from a verified,
encrypted backup without silently reactivating stale authority.

**Architecture:** `device` owns archive and recovery orchestration; a private
`device::archive` owns bounded encryption/framing and publication. Existing
`share::backup` supplies validated snapshots and new-workspace import.

**Tech Stack:** Rust, age 0.12.1, SQLite backup API, rustix, Windows MoveFileEx.

**Spec:** ../specs/2026-09-22-device-recovery-design.md

## Global Constraints

- New output only; never replace existing state, roots, key or backup files.
- Empty restored trust/grants, disabled jobs, fresh token and no service.
- All recovered folders receive-only and require full reconciliation before
  activation unless the operator explicitly chooses offline authority.
- Reuse strict checkpoint schema/path/object validation and preserve histories.
- Header <= 4 MiB, <= 128 folders, streamed objects, authenticated EOF required.
- Keep alpha designation and defer 100 GiB and physical-machine qualification.

## Review Focus

- Malicious authenticated payloads and aliases on case-insensitive filesystems.
- Concurrent config mutation or busy shares producing inconsistent authority.
- Deleted-but-pending materialization and stale send-only backups resurrecting data.
- Process death, ENOSPC and competing creation immediately before publication.
- Private key disclosure through errors, reports, temporary permissions or logs.

## Task 1: Encrypted capture and inspection

Files: create `src/device.rs`, `src/device/archive.rs`, `tests/device.rs`;
modify `Cargo.toml`, `Cargo.lock`, lib/main, identity, share and share/backup.

Interfaces: consume `Share::open/scan` and checkpoint validation; produce
`device::keygen(output)`, `backup(state,recipient,output)`, `inspect(backup,key)`
returning `Result<Value>`. Expose crate-only `share::backup::capture(&Share,path)`
and `verify(path)->Config`; `device::config_guard(state)` returns a held shared
file lock. Archive decode returns a private TempDir plus validated header.

- [x] Write CLI tests with a file changed after the last scan: keygen, backup,
  inspect, SHA-256 original source preservation, ciphertext without plaintext
  certificate/key/content, wrong key/truncation/tamper rejected, existing outputs
  preserved, source share/service locks rejected. Execute
  `cargo test --test device encrypted_capture`; expected missing-command RED.
- [x] Implement bounded header/entry encoding, authenticated full decode, age
  recipient-only encryption and private no-clobber output. Refactor existing
  checkpoint creation into capture plus CLI wrapper. Add configuration guards.
  Exact record sequence is `u32be path_len, path, u64be len, bytes`; path_len zero
  terminates and the next read must return authenticated EOF.
- [x] Run `cargo test --test device encrypted_capture`; expected GREEN. Run
  `cargo test --locked --all-targets`; expected complete green suite; commit.

## Task 2: Recovery and authority activation

Files: device module and archive publication; `src/share/backup.rs`, main CLI,
new process tests in `tests/device.rs`.

Interfaces: consume Task 1 decoded header/checkpoints; produce
`device::recover(backup,key,output,retired_device:Option<&str>)` and
`activate(state,folder,offline_authority)` returning `Result<Value>`.
`share::backup::import(checkpoint,new_state,staged_root,final_root)` materializes
the checkpoint under a fresh marker/epoch, empty grants, receive-only and hold.

- [x] Add tests recovering Unicode file, empty directory, modified retained
  history and a pending deletion. Assert new identity, empty peers, disabled
  jobs and remapped roots; default activation fails before reconcile. Run
  `cargo test --test device recovery`; expected missing-command RED.
- [x] Import only validated checkpoint materialized bytes excluding pending
  deletions; copy all objects/history. Write new configs and recovery report,
  publish whole private workspace atomically without replacement. Add explicit
  retired-identity matching and activation using original recorded mode.
- [x] Add real TCP/TLS test: A backs up; B later approves deletion; recover C,
  enroll B/C, full exchange, assert no resurrection by independent SHA-256 and
  file sets; activate then verify C edit reaches B and reverse edit reaches C.
- [x] Run `cargo test --test device` and full Rust suite; expected GREEN; commit.

## Task 3: Faults, review and delivery

Files: device regressions, docs/device-backup.md, README, docs/product-work.md.

Interfaces: consumes public CLI and staged publication; produces reproducible
operator walkthrough, review corrections and exact-head three-OS CI evidence.

- [x] Add authenticated malicious stream tests (path/duplicate/schema/hash),
  no-replace race and interrupted publication tests. Test production paths with
  owned fixture barriers, not environment-triggered production fault injection.
  Run focused regressions RED then fix and verify GREEN.
- [x] Write runnable backup/recover/enrollment/activation instructions; explain
  private key custody, restore quarantine, old identity retirement and leftovers.
- [ ] Run format, Clippy, all tests and obtain one independent final review.
  Important findings require reproduction/fix and affected/full verification.
- [ ] Commit and push the existing product-hardening branch, inspect all three
  Actions jobs, repair observed failures, and update Draft PR #1 with evidence.

Execution ruling: use the existing isolated checkout and native implementation
under the user's continuing authorization; do not add approval gates. Completing
this plan does not complete the full product goal.
