param(
  [Parameter(Mandatory=$true)][ValidateSet('install','rollback')][string]$Action,
  [string]$Package,
  [Parameter(Mandatory=$true)][string]$Prefix
)
$ErrorActionPreference = 'Stop'
function Test-VersionId([string]$Value) {
  return $Value -match '^[A-Za-z0-9][A-Za-z0-9._-]{0,159}$'
}
function Write-Pointer([string]$Name,[string]$Value) {
  $destination = Join-Path $Prefix $Name
  $temporary = Join-Path $Prefix ('.' + $Name + '-new')
  [IO.File]::WriteAllText($temporary, $Value + "`n")
  if ([IO.File]::Exists($destination)) { [IO.File]::Replace($temporary,$destination,$null) }
  else { [IO.File]::Move($temporary,$destination) }
}
function Switch-Version([string]$Next) {
  if (!(Test-VersionId $Next)) { throw 'Invalid version pointer' }
  if (!(Test-Path -LiteralPath (Join-Path $Prefix "versions/$Next/everywhere.exe") -PathType Leaf)) { throw 'Version binary is missing' }
  $current = Join-Path $Prefix 'current'
  if (Test-Path -LiteralPath $current) {
    $old = [IO.File]::ReadAllText($current).Trim()
    if (!(Test-VersionId $old)) { throw 'Invalid current pointer' }
    if ($old -eq $Next) { return }
    Write-Pointer 'previous' $old
  }
  Write-Pointer 'current' $Next
}
[IO.Directory]::CreateDirectory($Prefix) | Out-Null
$Prefix = (Resolve-Path -LiteralPath $Prefix).Path
$marker = Join-Path $Prefix '.everywhere-install'
if (!(Test-Path -LiteralPath $marker)) {
  if (@(Get-ChildItem -LiteralPath $Prefix -Force).Count -ne 0) { throw 'Prefix must be empty or an existing Everywhere installation' }
  [IO.File]::WriteAllText($marker, 'everywhere-install-v1')
}
if ([IO.File]::ReadAllText($marker).Trim() -ne 'everywhere-install-v1') { throw 'Unknown installation format' }
$lock = $null
try {
  $lock = [IO.File]::Open((Join-Path $Prefix '.install-lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
  if ($Action -eq 'rollback') {
    $previous = [IO.File]::ReadAllText((Join-Path $Prefix 'previous')).Trim()
    Switch-Version $previous
    Write-Output "Rolled back to $previous"
    return
  }
  if (!$Package) { throw 'Package is required' }
  $binary = Join-Path $Package 'everywhere.exe'
  $item = Get-Item -LiteralPath $binary
  if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Package binary must be a regular file' }
  $version = [IO.File]::ReadAllText((Join-Path $Package 'VERSION')).Trim()
  if (!(Test-VersionId $version) -or $version.Length -gt 64) { throw 'Invalid package version' }
  $expected = ([IO.File]::ReadAllText((Join-Path $Package 'SHA256SUMS')).Trim() -split '\s+')[0]
  if ($expected -notmatch '^[a-f0-9]{64}$') { throw 'Invalid SHA-256 checksum' }
  if ((Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'Package checksum mismatch' }
  $next = "$version-$expected"
  $versions = Join-Path $Prefix 'versions'
  [IO.Directory]::CreateDirectory($versions) | Out-Null
  $stage = Join-Path $versions ('.stage-' + [guid]::NewGuid().ToString('N'))
  [IO.Directory]::CreateDirectory($stage) | Out-Null
  $candidate = Join-Path $stage 'everywhere.exe'
  Copy-Item -LiteralPath $binary -Destination $candidate
  if ((Get-FileHash -LiteralPath $candidate -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'Copied binary checksum mismatch' }
  & $candidate --version | Out-Null
  if ($LASTEXITCODE -ne 0) { throw 'Candidate cannot run on this device' }
  $destination = Join-Path $versions $next
  if (Test-Path -LiteralPath $destination) {
    if ((Get-FileHash -LiteralPath (Join-Path $destination 'everywhere.exe') -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'Installed version is corrupt' }
    Remove-Item -LiteralPath $stage -Recurse
  } else { [IO.Directory]::Move($stage,$destination) }
  $launcher = @'
$ErrorActionPreference = 'Stop'
$version = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'current')).Trim()
if ($version -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,159}$') { throw 'Invalid installation pointer' }
& (Join-Path $PSScriptRoot "versions/$version/everywhere.exe") @args
exit $LASTEXITCODE
'@
  [IO.File]::WriteAllText((Join-Path $Prefix 'everywhere.ps1'),$launcher)
  Switch-Version $next
  Write-Output "Installed $version in $Prefix"
} finally { if ($null -ne $lock) { $lock.Dispose() } }
