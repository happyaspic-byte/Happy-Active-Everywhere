# Managed Soak Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans
> inline, with one fresh independent whole-change review before starting 72 hours.

**Goal:** Run three real managed peers through repeatable recovery/conflict cycles
and report only actually observed elapsed qualification.

**Architecture:** Separate the filesystem/version oracle, process/network helpers
and scenario controller. Existing CLI/API own all sync behavior. A private new
workspace and immutable executable preserve run provenance across later builds.

**Tech Stack:** Python 3.11+ standard library, existing Rust CLI, SQLite read-only
snapshots, loopback TCP/TLS, existing management API.

**Spec:** ../specs/2026-09-22-managed-soak-design.md

## Global Constraints

- Python 3.11+ standard library only; keep Rust dependencies unchanged.
- All data synthetic in one new private workspace; no existing shares or cleanup.
- No production fault-injection switches, native service installation or unmounts.
- Never log bearer tokens or upload whole private fixtures.
- Defaults: 120 seconds, two cycles, 60-second cycle interval, 90-second deadlines,
  30-second observation-gap limit, 4 GiB workspace cap, 2 GiB free-space reserve.
- Keep alpha; 72 hours means elapsed observed time, not accelerated iteration count.

## Review Focus

- A common corruption at all peers must fail the independent expected manifest.
- Pending deletes/conflicts or nonempty queues must not be hidden by matching files.
- A stale progress file, host suspend or controller death must never look passed.
- Killing a controller must close manager/worker ownership, without killing others.
- Existing workspaces, symlinks and failure artifacts must not expose keys or change user data.

## Task 1: Oracle, observation and process primitives

Files: create `scripts/soak_oracle.py`, `scripts/soak_runtime.py`,
`scripts/test_soak.py`.

Interfaces: `manifest(root)->dict[str,dict]`, `assert_manifest(root,expected)`,
`index_snapshot(state,folder)->dict`, `Observation(target,min_cycles,gap_limit)`
with `observe(monotonic,utc)`, `complete(monotonic,cycles)`;
`Relay(destination)` with address/counts/partition/close;
`Node(binary,base,name)` with `start/stop/api/cli/health/pause/resume`.

- [ ] Write negative tests before helpers: expected `note` SHA must reject equal
  corrupted bytes, extra paths and symlinks; an empty-file SHA must be accepted;
  `Observation(100,2,30)` at elapsed 99 with 100 cycles must not complete, and
  a 31-second observation gap must raise. Run
  `python3.11 -m unittest discover -s scripts -p test_soak.py -v`; expect missing
  helpers/then explicit negative-oracle RED.
- [ ] Implement independent pathlib/hashlib manifest and read-only SQLite snapshot
  including causal heads, pending, incoming/needed and history counts. Implement
  monotonic/UTC observation gate, private fixture ownership and bounded subprocess
  helpers. Relay only forwards/counts opaque TLS and closes owned sockets on faults.
- [ ] Test relay with real socket endpoints, verify counted bytes and connection
  partition; test lock owner liveness in a child process and after its termination.
  Test the same helper commands on a disposable CLI identity, never mocks of CLI.
- [ ] Run the Python tests and `cargo test --locked --all-targets`; expect GREEN.
  Commit `test: add independent managed-soak oracle and runtime helpers`.

## Task 2: Repeated managed three-peer scenarios

Files: create `scripts/soak.py`; extend `scripts/test_soak.py`; add
`docs/managed-soak.md`, README link and `.github/workflows/ci.yml` smoke step.

Interfaces: consume Task 1 helpers; expose
`python3.11 scripts/soak.py run --binary BIN --work-dir NEW --seconds N
--min-cycles N --cycle-interval N` and `status --work-dir RUN`.
Progress is atomic JSON; events are append-only and flushed; reports exclude keys.

- [ ] Add a failing CLI smoke/false-pass test: existing work directory must be
  rejected with its sentinel unchanged; short duration must not certify 72 hours;
  after controller termination, status must report interrupted and owned managers
  must exit. Execute the real runner CLI; expect missing-runner RED.
- [ ] Build three persistent managers and ring jobs via their authenticated API.
  Implement spec steps 1–6 using literal generated payloads and independent SHA-256,
  equality of normalized heads, exact pending/conflict expectations and empty queues.
  Use reviewed deletion versions for duplicate approval, and alternate reconnect order.
- [ ] Persist per-phase operation/result events and expected manifests. Add live
  status using a held lock, process identity and heartbeat age; refuse elapsed gaps.
  Record relay bytes, RSS and bounded storage. Snapshot/retain errors without secrets.
- [ ] Run `python3.11 scripts/soak.py run --binary target/debug/everywhere
  --work-dir folder-report/soak-smoke --seconds 10 --min-cycles 2 --cycle-interval 0`;
  expect two complete cycles, exact SHA-256 and zero unexpected revisions/queues,
  completed smoke with 72-hour qualification false. Run negative Python tests and
  full Rust suite; expect GREEN. CI uses a unique fresh workspace and uploads only
  `summary.json`, not fixture state or raw archives.
- [ ] Commit `test: exercise managed three-peer recovery and stability cycles`.

## Task 3: Review, delivery and actual elapsed qualification

Files: docs/managed-soak.md, docs/product-work.md; private run evidence and ledger.

Interfaces: consume current CI binary artifacts and Task 2 run/status commands;
produce exact-head all-OS smoke results, checked packages and a live 72-hour handle.

- [ ] Obtain one fresh independent review. Reproduce Critical/Important findings
  before fixing, verify each regression and run affected/full checks. Record review
  limitations; do not add a duplicate review after tested corrections.
- [ ] Commit/push `origin/codex/product-hardening`, inspect all three Actions jobs,
  fix actual failures and update Draft PR #1 with accurate smoke vs elapsed scope.
- [ ] Launch `run --seconds 259200 --min-cycles 2 --cycle-interval 60` using an
  immutable verified executable, in a new private local workspace. Record the live
  process/session and confirm status plus at least one complete scenario cycle.
  Expected: running, real elapsed progress, 72-hour qualification still false.
- [ ] Only after a live observed 72 hours, inspect final manifests/heads/queues,
  events, timing and process cleanup. Expected: complete 72-hour evidence or a
  specific retained failure. Keep this step pending while the real run is active.

Self-review: spec cycle requirements map to Task 2; observation and false-pass
guards to Task 1/2; delivery and actual elapsed evidence to Task 3. No approval
stall: existing user authorization covers native execution, synthetic tests and
pushes. A long run remains live while other full-product work continues.
