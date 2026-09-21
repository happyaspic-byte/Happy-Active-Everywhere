# Install and control a background service

Install a verified package and initialize its device state first. Approve peers,
register folders and save enabled synchronization jobs in the local dashboard.
The service runs that same manager and resumes its persisted jobs.

## Install and start

On macOS/Linux, use the installed launcher and your existing absolute state path:

```sh
"$HOME/.local/opt/everywhere/everywhere" service install \
  --state /absolute/path/everywhere-state \
  --prefix "$HOME/.local/opt/everywhere" \
  --listen 127.0.0.1:7445
```

In PowerShell 7 on Windows:

```powershell
& "$env:LOCALAPPDATA\Everywhere\everywhere.ps1" service install `
  --state "$env:LOCALAPPDATA\EverywhereState" `
  --prefix "$env:LOCALAPPDATA\Everywhere" `
  --listen 127.0.0.1:7445
```

`install` registers login startup, starts the service and waits for authenticated
manager health. Repeating it with the same settings is safe. A different state,
installation prefix or listen address requires uninstalling the old registration
first. The port must be nonzero and bound to a loopback address. Each state has
its own service identifier, so separate test devices do not share a registration.

- macOS uses a LaunchAgent in the current user's `~/Library/LaunchAgents` and
  requires that user's GUI login session.
- Linux uses a systemd user unit under `$XDG_CONFIG_HOME/systemd/user` (normally
  `~/.config/systemd/user`). A reachable user manager/bus is required. The product
  uses `systemctl` and `busctl` (including `ExecStartEx`; tested on systemd 255)
  to inspect the loaded executable, arguments and execution flags, and refuses
  units with drop-in overrides. `/usr/bin/env` executes literal installation
  paths because systemd restricts quotes in its executable token.
  does not enable linger, install a root unit or change machine login policy.
- Windows uses an interactive current-user Scheduled Task, with limited user
  privileges and no stored password. Windows PowerShell's ScheduledTasks module
  and the user's normal script execution policy must permit the local helper.
  An encoded PowerShell supervisor passes literal paths to the native bootstrap
  without Task Scheduler expanding `%VARIABLE%`; a parent pipe owns its lifetime.
  The supervisor retries a failed bootstrap after three seconds. In addition to
  login startup, a one-minute scheduled trigger recovers a killed supervisor;
  IgnoreNew leaves a healthy instance running. Stop disables both triggers.
  This is not a pre-login Windows system service.

OS file-access and background-item policies still apply to the actual account.
Test a disposable folder before registering important data. Unsupported user
sessions or denied access are reported as errors, not successful installation.

## Lifecycle and status

Use the installed launcher above, or `everywhere` when it is on PATH:

```sh
everywhere service status --state STATE
everywhere service stop --state STATE
everywhere service start --state STATE
everywhere service restart --state STATE
everywhere service uninstall --state STATE
```

`stop` stops the process and disables login startup until `start`. `restart`
stops it and starts the currently selected installed version. The native manager
restarts a failed service; the bootstrap's parent pipe prevents a killed service
from leaving its manager and synchronization workers running independently.

Status distinguishes:

- `installed`: a bound deployment manifest exists.
- `native.registered`, `native.enabled`, `native.running`: OS registration and
  runtime state; installation alone does not prove a running manager.
- `runner_live`: a bootstrap holds its state lock.
- `manager_lock_held`: a manager still holds the state lifetime lock; stop waits
  for both this lock and the bootstrap lock to be released.
- `healthy`: the live bootstrap's recorded manager PID answers authenticated
  health for this device. An unrelated foreground manager does not satisfy it.

Inspect `STATE/service/output.log` for manager diagnostics. It is private on Unix
and rotates to `output.previous.log` on startup after it exceeds 1 MiB. Individual
job errors and last successful synchronization remain in the dashboard/API.
Health does not prove a reachable peer or successful file transfer.

Uninstall removes only the owned native registration and deployment manifest.
It preserves device credentials, jobs, folders, history, checkpoints and service
logs. Repeating uninstall is safe. A changed/foreign definition or symlink is
refused rather than overwritten or stopped; restore the intended registration
before retrying. Never delete unrelated service-manager entries to repair one.
Ownership checks include the loaded command, not only the registration's file
path. Scheduler access/query errors preserve the deployment manifest.

## Updates and rollback

```sh
everywhere service stop --state STATE
sh NEW_PACKAGE/scripts/install.sh install NEW_PACKAGE INSTALL_PREFIX
everywhere service start --state STATE
```

Use `install.ps1` on Windows. For rollback, stop the service, run the existing
installer's `rollback` action, and start it again. The retained bootstrap resolves
and verifies the installer's `current` pointer on every launch, so restarts use
the selected manager version. Do not remove the bootstrap's retained version:
its path is recorded in `STATE/service/config.json`. Uninstall/reinstall the
service before removing old versions or changing the deployment layout.
For services installed by an earlier intermediate review build on Linux or
Windows, uninstall using that retained build before installing a changed service
definition. Those initial native definitions are not a stable migration format.

Rolling back across an incompatible state/protocol change is not a data rollback.
In particular, complete recovery reconciliation before using an older build that
does not understand recovery holds. User data needs its own protected backup.

## Reproduce native acceptance

```sh
python3 scripts/verify-service.py --binary target/debug/everywhere \
  --report folder-report/service-local.json
```

The test uses unique temporary devices and a real native registration. It checks
install/start/stop/restart/uninstall, foreign-registration refusal, installed
version update/rollback, paused-job preservation, actual manager crash/restart
and bidirectional TLS content with independent SHA-256. Paths contain literal
`$HOME` and `%USERNAME%`, Unicode, spaces and apostrophes. POSIX tests load a
foreign command while retaining the original source bytes and require refusal;
stop/uninstall also check the manager and active worker PIDs. Windows query-error
classification has a separate injected-boundary test, not an OS ACL test.
On POSIX the stop case suspends its worker first and checks termination before
resuming any survivor in failure cleanup.
Windows also kills the uniquely identified disposable task supervisor and checks
that its children exit, the scheduled trigger recreates it and TLS sync resumes.
The native test removes its own
registration in `finally`; failed fixtures and bounded diagnostic reports remain
for inspection. CI explicitly prepares a user manager on disposable Linux
runners if needed; that runner setup is not production deployment behavior.

These tests exercise service/process restarts on the tested account. They do not
reboot the OS, log the account out/in, or establish physical NAS/WAN or elapsed
72-hour/7-day stability. The product remains an alpha pending those gates.
