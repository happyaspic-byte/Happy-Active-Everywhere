# User Service Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans
> to implement this plan task-by-task with one independent final review.

**Goal:** Install and control a verified per-user background service that resumes
real synchronization after process restart.

**Architecture:** `service` owns validated deployment state and the bootstrap;
`service::native` owns platform registration/control. Management health and
parent-pipe shutdown connect the service to the existing job supervisor.

**Tech Stack:** Rust, existing SQLite/TLS/job engine, native launchctl/systemctl,
Windows PowerShell ScheduledTasks, Python acceptance harness.

**Spec:** ../specs/2026-09-22-user-service-design.md

## Global Constraints

- Per-user service only; no root service, stored passwords, or automatic linger.
- Preserve current credentials, grants, jobs, user data, history and checkpoints.
- Fixed nonzero loopback management address; no credentials in registration.
- Verify selected installed binary SHA-256; preserve the retained bootstrap.
- Reject foreign registrations, links, corrupt pointers, and state/prefix overlap.
- Native acceptance uses disposable registrations and cleans them up explicitly.
- Keep the alpha classification; physical login/reboot and soak are separate.

## Review Focus

- Native format escaping of spaces, quotes, Unicode, dollar and percent signs.
- Killing the bootstrap must not orphan the manager or synchronization children.
- A busy foreground manager must not be mistaken for a healthy owned service.
- Interrupted install/uninstall and manually changed native registrations.
- Installer pointer replacement must select the new version without losing jobs.

## Task 1: Verified bootstrap and manager lifecycle

Files: create `src/service.rs`, `tests/service.rs`; modify `src/lib.rs`,
`src/main.rs`, `src/management.rs`, `Cargo.toml`.

Consumes: existing install prefix marker/current/versions, `management::serve`,
`jobs::watch_parent`. Produces `service::configure(state, prefix, listen)` and
`service::run(state)`, authenticated `/api/health`, and `service run --state`.

- [x] Write actual CLI regressions that build a disposable checksum-bearing
  installer prefix, call configure, launch `service run`, and inspect health.
  Start a real approved TLS peer and verify literal payload SHA-256 after job
  startup, runner death and restart. Preserve a paused job and switch versions.
  Reject duplicate runners, tampered binary/pointer, nonloopback addresses,
  overlapping paths and symlinked service configuration.
- [x] Run `cargo test --locked --test service`; expected RED: absent service API
  or CLI. Implement manifest validation, locks, verified current selection,
  startup log rotation, parent pipe ownership, health and PID ownership checks.
- [x] Run `cargo test --locked --test service`; expected GREEN. Run complete
  Rust tests before recording Task 1 complete and commit the coherent bootstrap.

## Task 2: Native registration and lifecycle CLI

Files: create `src/service/native.rs`, `src/service/task.ps1`,
`scripts/verify-service.py`; modify main and service module.

Consumes: Task 1 manifest/bootstrap/health. Produces install/start/stop/restart/
status/uninstall with native ownership validation and bounded command execution.

- [ ] Add a real native acceptance script using a disposable prefix and two
  initialized peers. It executes install/status/stop/start/restart/uninstall,
  validates independently hashed bidirectional delivery after restart, and
  leaves foreign registration/data fixtures untouched. It always unregisters
  its owned service in `finally` and retains diagnostic logs on failure.
- [ ] Run the script on the current Mac; expected RED: missing lifecycle
  commands. Implement launchd, systemd and ScheduledTasks adapters with exact
  ownership checks, native escaping and explicit manager-unavailable errors.
- [ ] Run the script again; expected GREEN on the current supported user session.
  Add bounded Linux/Windows CI setup only for disposable hosted runners.
  Missing native sessions must be reported and resolved, never silently skipped.

## Task 3: Delivery and independent review

Files: update `docs/services.md`, `docs/install-update.md`, README,
`docs/product-work.md`, `.github/workflows/ci.yml`, package script if needed.

Consumes: both task interfaces. Produces packaged CLI, runnable operator
instructions, failed-test artifacts, review corrections and exact-head CI proof.

- [ ] Document lifecycle and update commands, retained bootstrap ownership,
  login/session requirements, status meanings and physical-reboot limitations.
- [ ] Run format, Clippy, all Rust tests, release compilation and native
  acceptance. Obtain one independent final review; reproduce important findings
  RED before fixes and re-run the affected/full checks as required.
- [ ] Commit, fast-forward push to `codex/product-hardening`, inspect all three
  OS Actions jobs, fix observed failures and update Draft PR #1 with actual
  results. Do not merge or publish a stable release while product gates remain.

Execution: native implementation in the already isolated worktree under the
user's persistent authorization. Design/plan structure does not add a new
approval stop to the authorized autonomous task. The full product goal remains
active after this subsystem; this plan is not a redefinition of completion.
