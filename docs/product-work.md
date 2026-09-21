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
