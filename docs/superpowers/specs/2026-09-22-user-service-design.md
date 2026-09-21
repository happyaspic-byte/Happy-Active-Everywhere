# User service lifecycle

The product must keep approved synchronization jobs running after login and
recover from a manager exit. The existing manager, jobs, authentication and
installer remain authoritative. This is a new deployment subsystem within the
user's authorized autonomous product-completion goal.

## Selected design

Use native per-user services: launchd LaunchAgents on macOS, systemd user units
on Linux, and an interactive current-user Scheduled Task on Windows. A custom
detached daemon would duplicate OS supervision and lose login integration;
machine services would require new administrator privileges. Neither is needed
for this personal product. No root service, password storage or automatic
enabling of Linux linger is introduced.

`everywhere service install --state STATE --prefix PREFIX --listen ADDRESS`
registers, enables and starts the service. The state must already be initialized,
the prefix must be a verified installer prefix, and the address must be a fixed
nonzero loopback address. `start`, `stop`, `restart`, `status`, and `uninstall`
take `--state`. Stop disables login startup until start is requested. Uninstall
removes only the owned native registration and service manifest, preserving
credentials, jobs, user files, history, checkpoints and logs.

The installation records a private, versioned manifest bound to the canonical
state path and device identity. A per-state identifier avoids collisions between
test and real devices. An existing mismatching native registration is never
overwritten or stopped. Lifecycle changes use a state lock and atomic manifest
publication; interrupted installation can be retried with identical settings.
Native command failures/timeouts must be reported, not interpreted as success.

The registration starts a retained installed executable as a small bootstrap.
`service run` reads the current installer pointer on each launch, verifies its
SHA-256, then starts that version's manager with a parent-owned stdin pipe.
Thus an update or rollback selects the right manager after service restart.
The retained bootstrap version must stay installed; future incompatible service
protocols require reinstalling the registration. A manager's pipe EOF stops it;
existing job pipes then stop its children. Only one runner can own a state.
The OS restarts failed runners. Logs stay private and rotate on startup.

Status separates registration, enabled/native-running state, live runner and
authenticated manager health. A new small authenticated `/api/health` response
identifies the device and process without scanning folders. The runner records
the child PID; a responding unrelated foreground manager must not count as a
healthy service. Startup waits for owned manager health with a bounded deadline.

## Safety and portability

State and install prefix may contain spaces, Unicode, quotes, percent signs and
dollar signs. Native definitions quote their own formats, not a shell. Reject
control characters, relative deployment paths after normalization, symlinked
service files, invalid installation pointers and corrupt binaries. State/prefix
overlap is refused. No credential appears in registration, arguments or output.
Unix service files/logs use private modes. Windows inherits the user's ACLs;
explicit ACL hardening remains a separate qualification item.

The current user's real home and native service manager are used only by explicit
service lifecycle commands. Library/configuration tests write isolated fixtures.
Native acceptance uses a unique disposable state/registration, cleans up that
registration in `finally`, and never touches a real existing service. Missing
user sessions/buses are an explicit unsupported test environment, not a pass.

## Evidence required

1. A real CLI bootstrap reads a verified installation, serves authenticated
   health, resumes an enabled TLS job, and transfers independently hashed bytes.
2. Killing the runner ends its manager and jobs; restarting loads the selected
   installed version and preserves paused/enabled jobs and synchronization data.
3. Duplicate runners, corrupt pointers/binaries, nonloopback addresses and
   foreign/symlinked service files fail without changing unrelated state.
4. On every hosted OS, native install/start/status/stop/restart/uninstall performs
   real process control and bidirectional file delivery after restart. Definition
   parsing uses native tools; no mocked service manager is accepted as OS proof.
5. Existing 77 Unix / 68 Windows tests, installers, browser, APFS fault tests,
   release builds and packages remain green at the exact pushed head.

Service restart is not an OS reboot test. Physical account login, NAS/WAN,
72-hour/7-day soak and 100 GiB qualification remain distinct outstanding gates.

Native format references: [Apple launchd guide](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html),
[systemd service documentation](https://github.com/systemd/systemd/blob/main/man/systemd.service.xml),
[Microsoft Task Scheduler schema](https://learn.microsoft.com/en-us/windows/win32/taskschd/task-scheduler-schema).
