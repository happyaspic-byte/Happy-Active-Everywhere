param(
  [Parameter(Mandatory=$true)][ValidateSet('install','start','stop','status','uninstall')][string]$Action,
  [Parameter(Mandatory=$true)][string]$Config
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$settings = Get-Content -LiteralPath $Config -Raw -Encoding UTF8 | ConvertFrom-Json
$userSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
# Scheduler expands %VARIABLE% even inside quoted arguments. Encode user paths
# as data and start the bootstrap without shell expansion. Keep stdin open so
# terminating the scheduled wrapper also terminates the runner and its children.
$exeData = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($settings.executable))
$argData = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($settings.arguments))
$wrapper = @'
$ErrorActionPreference='Stop'
$info=New-Object Diagnostics.ProcessStartInfo
$info.FileName=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__EXE__'))
$info.Arguments=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__ARGS__'))
$info.UseShellExecute=$false
$info.RedirectStandardInput=$true
$process=New-Object Diagnostics.Process
$process.StartInfo=$info
try {
  if (!$process.Start()) { throw 'Bootstrap did not start' }
  $process.WaitForExit()
  exit 1
} finally {
  if (!$process.HasExited) { $process.Kill(); $process.WaitForExit() }
  $process.Dispose()
}
'@
$wrapper = $wrapper.Replace('__EXE__',$exeData).Replace('__ARGS__',$argData)
$nativeExe = Join-Path $PSHOME 'powershell.exe'
$nativeArgs = '-NoProfile -NonInteractive -EncodedCommand ' + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($wrapper))
function Find-Task {
  try {
    return Get-ScheduledTask -TaskName $settings.id -TaskPath '\' -ErrorAction Stop
  } catch {
    if ($_.CategoryInfo.Category -eq 'ObjectNotFound' -and
        $_.FullyQualifiedErrorId.Split(',')[0] -eq 'CmdletizationQuery_NotFound_TaskName') { return $null }
    throw
  }
}
$existing = Find-Task
if ($existing) {
  $actions = @($existing.Actions)
  if ($existing.Description -cne $settings.owner -or $actions.Count -ne 1 -or
      $actions[0].Execute -cne $nativeExe -or $actions[0].Arguments -cne $nativeArgs -or
      $existing.Principal.UserId -cne $userSid -or $existing.Principal.RunLevel.ToString() -ne 'Limited' -or
      $existing.Principal.LogonType.ToString() -ne 'Interactive') {
    throw 'Scheduled task belongs to another deployment or was changed; preserving it'
  }
}
if ($Action -eq 'install' -and !$existing) {
  $nativeAction = New-ScheduledTaskAction -Execute $nativeExe -Argument $nativeArgs
  $trigger = New-ScheduledTaskTrigger -AtLogOn -User $userSid
  $principal = New-ScheduledTaskPrincipal -UserId $userSid -LogonType Interactive -RunLevel Limited
  $options = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -RestartInterval (New-TimeSpan -Minutes 1) -RestartCount 999
  Register-ScheduledTask -TaskName $settings.id -TaskPath '\' -Action $nativeAction -Trigger $trigger -Principal $principal -Settings $options -Description $settings.owner | Out-Null
  $existing = Find-Task
}
if ($Action -eq 'start') {
  if (!$existing) { throw 'Service task is not registered; install it first' }
  Enable-ScheduledTask -TaskName $settings.id -TaskPath '\' | Out-Null
  Start-ScheduledTask -TaskName $settings.id -TaskPath '\'
}
if (($Action -eq 'stop' -or $Action -eq 'uninstall') -and $existing) {
  Disable-ScheduledTask -TaskName $settings.id -TaskPath '\' | Out-Null
  Stop-ScheduledTask -TaskName $settings.id -TaskPath '\'
}
if ($Action -eq 'uninstall' -and $existing) {
  Unregister-ScheduledTask -TaskName $settings.id -TaskPath '\' -Confirm:$false
}
$task = Find-Task
@{ backend='task-scheduler'; registered=($null -ne $task); enabled=($null -ne $task -and $task.Settings.Enabled); running=($null -ne $task -and $task.State.ToString() -eq 'Running'); task_name=$settings.id } | ConvertTo-Json -Compress
