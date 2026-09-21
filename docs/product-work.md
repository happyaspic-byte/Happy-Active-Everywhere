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
- Added the baseline three-OS workflow. Its outcome is pending.

## Completion boundaries

Hosted runner tests do not establish compatibility with the user's physical
devices, Synology model, storage mounts or real WAN. Long-duration tests require
their actual elapsed duration. Those gates remain unverified until executed.

## Design rulings

- Keep the current alpha classification until functional and safety gates pass.
  A build artifact is not a stable release.
- Use a dedicated clone and branch as the isolated workspace; an additional
  nested worktree would provide no protection beyond this fresh checkout.
