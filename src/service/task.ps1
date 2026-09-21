param(
  [Parameter(Mandatory=$true)][ValidateSet('install','start','stop','status','uninstall')][string]$Action,
  [Parameter(Mandatory=$true)][string]$Config
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$settings = Get-Content -LiteralPath $Config -Raw -Encoding UTF8 | ConvertFrom-Json
$userSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$existing = Get-ScheduledTask -TaskName $settings.id -TaskPath '\' -ErrorAction SilentlyContinue
if ($existing) {
  $actions = @($existing.Actions)
  if ($existing.Description -cne $settings.owner -or $actions.Count -ne 1 -or
      $actions[0].Execute -cne $settings.executable -or $actions[0].Arguments -cne $settings.arguments -or
      $existing.Principal.UserId -cne $userSid -or $existing.Principal.RunLevel.ToString() -ne 'Limited' -or
      $existing.Principal.LogonType.ToString() -ne 'Interactive') {
    throw 'Scheduled task belongs to another deployment or was changed; preserving it'
  }
}
if ($Action -eq 'install' -and !$existing) {
  $nativeAction = New-ScheduledTaskAction -Execute $settings.executable -Argument $settings.arguments
  $trigger = New-ScheduledTaskTrigger -AtLogOn -User $userSid
  $principal = New-ScheduledTaskPrincipal -UserId $userSid -LogonType Interactive -RunLevel Limited
  $options = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -RestartInterval (New-TimeSpan -Minutes 1) -RestartCount 999
  Register-ScheduledTask -TaskName $settings.id -TaskPath '\' -Action $nativeAction -Trigger $trigger -Principal $principal -Settings $options -Description $settings.owner | Out-Null
  $existing = Get-ScheduledTask -TaskName $settings.id -TaskPath '\'
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
$task = Get-ScheduledTask -TaskName $settings.id -TaskPath '\' -ErrorAction SilentlyContinue
@{ backend='task-scheduler'; registered=($null -ne $task); enabled=($null -ne $task -and $task.Settings.Enabled); running=($null -ne $task -and $task.State.ToString() -eq 'Running'); task_name=$settings.id } | ConvertTo-Json -Compress
