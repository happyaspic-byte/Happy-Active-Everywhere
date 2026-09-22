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
while ($true) {
  $process=New-Object Diagnostics.Process
  $process.StartInfo=$info
  try {
    if (!$process.Start()) { throw 'Bootstrap did not start' }
    $process.WaitForExit()
  } finally {
    if (!$process.HasExited) { $process.Kill(); $process.WaitForExit() }
    $process.Dispose()
  }
  Start-Sleep -Seconds 3
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
        $_.FullyQualifiedErrorId.Split(',')[0] -in @('CmdletizationQuery_NotFound_TaskName','CmdletizationQuery_NotFound')) { return $null }
    throw
  }
}
$existing = Find-Task
if ($existing) {
  $actions = @($existing.Actions)
  # Scheduler may return an account name even when registration used its SID.
  $principalId=$existing.Principal.UserId
  if ($principalId -match '^S-\d-') {
    $principalSid=(New-Object Security.Principal.SecurityIdentifier($principalId)).Value
  } else {
    $account=New-Object Security.Principal.NTAccount($principalId)
    $principalSid=$account.Translate([Security.Principal.SecurityIdentifier]).Value
  }
  $mismatch=@()
  if ($existing.Description -cne $settings.owner) { $mismatch+='owner marker' }
  if ($actions.Count -ne 1) { $mismatch+='action count' }
  elseif ($actions[0].Execute -cne $nativeExe -or $actions[0].Arguments -cne $nativeArgs) { $mismatch+='executable or arguments' }
  if ($principalSid -cne $userSid) { $mismatch+='user SID' }
  if ($existing.Principal.RunLevel.ToString() -ne 'Limited') { $mismatch+='run level' }
  if ($existing.Principal.LogonType.ToString() -ne 'Interactive') { $mismatch+='logon type' }
  if ($mismatch.Count) { throw ('Scheduled task ownership mismatch ('+($mismatch -join ', ')+'); preserving it') }
}
if ($Action -eq 'install' -and !$existing) {
  $nativeAction = New-ScheduledTaskAction -Execute $nativeExe -Argument $nativeArgs
  $loginTrigger = New-ScheduledTaskTrigger -AtLogOn -User $userSid
  # A repeating trigger also recovers an externally killed wrapper; IgnoreNew
  # leaves a healthy instance alone. Disabled tasks do not run either trigger.
  $healthTrigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 1)
  $principal = New-ScheduledTaskPrincipal -UserId $userSid -LogonType Interactive -RunLevel Limited
  $options = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -RestartInterval (New-TimeSpan -Minutes 1) -RestartCount 999
  Register-ScheduledTask -TaskName $settings.id -TaskPath '\' -Action $nativeAction -Trigger @($loginTrigger,$healthTrigger) -Principal $principal -Settings $options -Description $settings.owner | Out-Null
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
