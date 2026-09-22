param([Parameter(Mandatory=$true)][string]$Helper)
$ErrorActionPreference='Stop'
# Inject only the external Task Scheduler query boundary. Real registration,
# %VARIABLE% paths, execution and cleanup are covered by verify-service.py.
$fixture=Join-Path ([IO.Path]::GetTempPath()) ('everywhere-query-'+[Guid]::NewGuid())
New-Item -ItemType Directory -Path $fixture | Out-Null
$config=Join-Path $fixture 'request.json'
@{id='query-test';executable='C:\fixture\everywhere.exe';arguments='service run';owner='test'} |
  ConvertTo-Json | Set-Content -LiteralPath $config -Encoding UTF8
function Get-ScheduledTask {
  [CmdletBinding()]param([string]$TaskName,[string]$TaskPath)
  $global:queryCount++
  if ($global:queryMode -like 'missing*' -or ($global:queryMode -eq 'second' -and $global:queryCount -eq 1)) {
    $errorId=if ($global:queryMode -eq 'missing-combined') {'CmdletizationQuery_NotFound'} else {'CmdletizationQuery_NotFound_TaskName'}
    Write-Error 'No matching task' -Category ObjectNotFound -ErrorId $errorId
  } else {
    Write-Error 'Injected scheduler access denial' -Category PermissionDenied -ErrorId SchedulerAccessDenied
  }
}
try {
  foreach ($mode in @('denied','second')) {
    $global:queryMode=$mode; $global:queryCount=0; $rejected=$false
    try { & $Helper -Action uninstall -Config $config | Out-Null }
    catch {
      if ($_.ToString() -notmatch 'Injected scheduler access denial') { throw }
      $rejected=$true
    }
    if (!$rejected) { throw "Scheduler $mode query failure was mistaken for missing registration" }
  }
  foreach ($mode in @('missing','missing-combined')) {
    $global:queryMode=$mode; $global:queryCount=0
    $status=& $Helper -Action uninstall -Config $config | ConvertFrom-Json
    if ($status.registered -ne $false) { throw "Exact $mode task-not-found was not accepted" }
  }
  Write-Output 'Task query error classification passed (injected external boundary)'
} finally {
  Remove-Item -LiteralPath $fixture -Recurse -Force
}
