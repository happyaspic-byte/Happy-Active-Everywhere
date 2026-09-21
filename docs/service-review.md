# Service review corrections

Review base: `da3fad8..5d7bf3f`. The first native-service CI run,
[35647941177](https://github.com/happyaspic-byte/Happy-Active-Everywhere/actions/runs/35647941177),
passed macOS but failed Linux service startup and Windows bootstrap status.

| Finding | Correction | Evidence / boundary |
| --- | --- | --- |
| Linux executable `$` escaped as `$$` | Use systemd's `:` prefix to disable environment expansion, preserving literal dollars; escape `%` specifiers separately | Initial hosted Linux startup failed; literal-variable-path acceptance runs on the corrected head |
| Source path alone treated as native ownership | macOS compares loaded program and argument vector; Linux reads typed effective ExecStart over the user bus and rejects drop-ins | Local macOS RED stopped a disposable foreign `/bin/sleep`; corrected native acceptance preserves it. Linux equivalent runs in CI |
| Windows lock contention classified as an error | Compare the OS error against fs2's platform lock-contention code | Hosted Windows returned error 33 from local_status while its bootstrap was running; existing independent-process test exercises the correction |
| Scheduler query errors treated as missing task | Only the precise CIM task-name-not-found error is absence; all other errors propagate | Windows test injects denied initial and final queries at the native query boundary; actual Windows ACL denial remains unverified |
| Scheduler expands paired `%VARIABLE%` paths | Encoded current-user PowerShell wrapper starts bootstrap with literal arguments and an owned stdin pipe | Native fixture uses literal `%USERNAME%` in both state and install paths; real scheduler verification runs in CI |
| Install released its lock after configure | One lifecycle lock spans configuration, registration, start and reporting | Structural correction to the reviewed interleaving; no claim of deterministic reproduction of the original narrow gap |
| Stop could precede manager lock release | Wait for both manager and bootstrap locks; assert managed PIDs are gone after native stop/uninstall | Local native acceptance also stops a worker suspended with SIGSTOP, verifies its death before any resume, and passes; Windows suspension and physical shutdown remain separate qualification work |

The next hosted run (`35649465434`) passed all applicable Rust tests (including
Windows scheduler query-error injection), then exposed two native-adapter issues:
systemd could collect an inactive unit between GetUnit and property inspection;
Windows PowerShell rejected an extended `\\?\` helper script path. The follow-up
queries the reversible unit object path directly (loading it in the same property
request), and passes normal literal Windows paths to PowerShell without changing
execution policy. These changes require their own exact-head native acceptance.

A local restart test also exposed an incorrect test precondition: visible bytes
could precede DB acknowledgement. Killing there and editing while stopped left
two preserved, concurrent revisions, so asserting a particular primary filename
was invalid. This completed-sync restart test now waits for the managed job's
successful exchange before stopping it; the separate interruption tests continue
to validate every retained revision. Failure SQLite evidence confirmed both
original and edited hashes were preserved. The bootstrap test additionally closes
its parent pipe and verifies runner, manager and worker termination.

The local foreign-task test creates its own unique label, restores the definition
and removes only that registration in cleanup. It never changes another user's
jobs. A native-service restart is not proof of an OS reboot or login cycle.
