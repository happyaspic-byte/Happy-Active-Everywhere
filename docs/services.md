# Run management at login

These are per-user deployment instructions. CI verifies the executable and
installer, but it has not verified startup on your physical account/device.
Initialize the state directory and approve peers before enabling background
jobs. Use the installed launcher so updates select the new version after a
service restart. Stop the service before updating or rolling back.

## Linux: systemd user service

Create `~/.config/systemd/user/everywhere.service`:

```ini
[Unit]
Description=Happy Active Everywhere personal synchronization
After=network-online.target

[Service]
Type=simple
ExecStart=%h/.local/opt/everywhere/everywhere manage --state %h/.local/state/everywhere --listen 127.0.0.1:7445
Restart=on-failure
RestartSec=5
TimeoutStopSec=30
KillMode=control-group
UMask=0077

[Install]
WantedBy=default.target
```

Adapt the paths if your installation differs. For paths containing spaces,
quote the executable and each path argument. Validate and enable:

```sh
systemd-analyze --user verify ~/.config/systemd/user/everywhere.service
systemctl --user daemon-reload
systemctl --user enable --now everywhere.service
systemctl --user status everywhere.service
journalctl --user -u everywhere.service
```

Stop before updates: `systemctl --user stop everywhere.service`.
Remove autostart: `systemctl --user disable --now everywhere.service`.
This is a user-login service; running while logged out requires an explicit
host-level service/linger policy configured by the machine owner.

## macOS: LaunchAgent

Create `~/Library/LaunchAgents/local.happy.everywhere.plist`, replacing both
`/Users/YOUR_USER` paths with your real absolute paths:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>local.happy.everywhere</string>
  <key>ProgramArguments</key><array>
    <string>/Users/YOUR_USER/.local/opt/everywhere/everywhere</string>
    <string>manage</string><string>--state</string>
    <string>/Users/YOUR_USER/.local/state/everywhere</string>
    <string>--listen</string><string>127.0.0.1:7445</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>5</integer>
</dict></plist>
```

```sh
plutil -lint ~/Library/LaunchAgents/local.happy.everywhere.plist
launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/local.happy.everywhere.plist
launchctl print "gui/$(id -u)/local.happy.everywhere"
```

Stop before updates with:

```sh
launchctl bootout "gui/$(id -u)" ~/Library/LaunchAgents/local.happy.everywhere.plist
```

Bootstrap again after updating. To uninstall autostart, boot out and remove
only this plist. The OS may request permission to access protected user
folders; grant only the folders you intended to synchronize.

## Windows: Task Scheduler, current user

Run these in PowerShell 7 after changing the state path to your initialized
device state:

```powershell
$EverywhereLauncher = "$env:LOCALAPPDATA\Everywhere\everywhere.ps1"
$EverywhereState = "$env:LOCALAPPDATA\EverywhereState"
$EverywhereUser = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$EverywhereAction = New-ScheduledTaskAction -Execute (Get-Command pwsh).Source -Argument ('-NoProfile -File "' + $EverywhereLauncher + '" manage --state "' + $EverywhereState + '" --listen 127.0.0.1:7445')
$EverywhereTrigger = New-ScheduledTaskTrigger -AtLogOn -User $EverywhereUser
$EverywherePrincipal = New-ScheduledTaskPrincipal -UserId $EverywhereUser -LogonType Interactive -RunLevel Limited
$EverywhereSettings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName 'Happy Active Everywhere' -Action $EverywhereAction -Trigger $EverywhereTrigger -Principal $EverywherePrincipal -Settings $EverywhereSettings
Start-ScheduledTask -TaskName 'Happy Active Everywhere'
Get-ScheduledTaskInfo -TaskName 'Happy Active Everywhere'
```

Before an update, use `Stop-ScheduledTask -TaskName 'Happy Active Everywhere'`.
To remove autostart, stop the task and run
`Unregister-ScheduledTask -TaskName 'Happy Active Everywhere'`.
The task uses the current user's permissions, not administrator privileges.
PowerShell must be installed and the host's script execution policy must allow
the local installed launcher.

## Verify after restart

Open `http://127.0.0.1:7445`, log in with the explicitly requested management
token and inspect job status. Test one disposable file in both directions and
verify its bytes. An enabled service or running job alone does not demonstrate
successful peer authentication, writable mounts or convergence.
