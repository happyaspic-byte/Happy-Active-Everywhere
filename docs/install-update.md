# Install, update and roll back a verified build

Download the OS/architecture package from an **all-green** workflow run on the
work branch. Artifacts currently expire after 14 days. They are alpha packages,
not signed stable releases. Compare the ZIP's SHA-256 with its `.sha256` sidecar
before extraction. A same-source checksum is integrity checking, not an
independent publisher signature.

Extract the ZIP. Its top-level directory contains `everywhere` (or
`everywhere.exe`), `VERSION`, `SHA256SUMS`, `build.json`, docs and installers.
The host architecture is recorded in the filename and `build.json`; do not
assume an ARM package runs on x86 or on a Synology NAS.

When building packages from source, `scripts/package.py` requires Python 3.11
or newer (`tomllib`). This is a packaging-tool requirement; the installed
Rust executable does not require Python.

## Linux and macOS

```sh
sh PACKAGE/scripts/install.sh install PACKAGE "$HOME/.local/opt/everywhere"
"$HOME/.local/opt/everywhere/everywhere" --version
```

Use an empty destination for the first installation. The installer checks the
binary checksum, copies it into a version-specific directory, runs `--version`
and switches the `current` pointer. A prefix containing spaces is supported.
The installer refuses unrelated nonempty directories. Add the prefix to PATH
if desired; it does not change shell configuration automatically.

To update, stop the management service, run the same install command with the
new extracted package, then start management again. Existing device state and
synchronized folders are not touched by the installer.

```sh
sh PACKAGE/scripts/install.sh rollback "$HOME/.local/opt/everywhere"
```

Rollback switches to the previous retained executable. It does not reverse
file edits, remove history, or migrate an incompatible database backward. If
a later release changes the state schema, use that release's migration and
backup instructions before downgrading. Interrupted POSIX installation leaves
`.install-lock`; confirm no installer is running before removing that empty
lock directory and retrying. Candidate staging directories are harmless and
are not referenced by the current launcher.

## Windows (PowerShell)

```powershell
pwsh -File PACKAGE/scripts/install.ps1 -Action install -Package PACKAGE -Prefix "$env:LOCALAPPDATA\Everywhere"
& "$env:LOCALAPPDATA\Everywhere\everywhere.ps1" --version
pwsh -File PACKAGE/scripts/install.ps1 -Action rollback -Prefix "$env:LOCALAPPDATA\Everywhere"
```

Use your organization's script execution policy. The test environment uses
PowerShell 7. Windows executable replacement is avoided by retaining versioned
directories and switching a small pointer file. Stop management before updating
so its supervised jobs use the selected version after restart.

## Start management

```sh
everywhere management-token --state /absolute/path/device-state
everywhere manage --state /absolute/path/device-state --listen 127.0.0.1:7445
```

Open the exact loopback IP and port shown above. The server intentionally
rejects alternate Host headers and non-loopback listeners. Registered jobs
resume according to their persisted enabled/paused state. A running process
is not evidence of a reachable peer; inspect the last successful iteration
and latest result. Device authorization and folder grants are still required.

For login startup and native supervision, use `everywhere service install` with
this installation prefix and initialized state. See [service lifecycle](services.md)
for install/start/stop/status/uninstall, update/rollback and user-session requirements.
Stop the service before updating or removing executables. Service uninstall keeps
device state and synchronized data; remove only the intended installation prefix
when uninstalling binaries. Preserve the registered bootstrap version until the
service is uninstalled or re-registered.
