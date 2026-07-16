#Requires -Version 5.1
<#
.SYNOPSIS
  Install MPGS server as a Windows Service via WinSW.

.DESCRIPTION
  Expects a release package layout produced by scripts/package_server.ps1:
    package\
      bin\mpgs-server.exe
      bin\mpgs-dbtool.exe
      windows\mpgs-server.xml
      windows\winsw.exe   (optional; download if missing)

  Does NOT download certificates or perform code signing.

.PARAMETER PackageRoot
  Path to the extracted package root.

.PARAMETER WinswPath
  Path to winsw executable. Defaults to PackageRoot\windows\winsw.exe.

.PARAMETER ServiceName
  Windows service name (default mpgs-server).
#>
param(
    [Parameter(Mandatory = $true)][string]$PackageRoot,
    [string]$WinswPath = '',
    [string]$ServiceName = 'mpgs-server'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$PackageRoot = (Resolve-Path -LiteralPath $PackageRoot).Path
$serverExe = Join-Path $PackageRoot 'bin\mpgs-server.exe'
$xmlSrc = Join-Path $PackageRoot 'windows\mpgs-server.xml'
if (-not (Test-Path -LiteralPath $serverExe)) {
    throw "missing $serverExe — build a package with scripts/package_server.ps1 first"
}
if (-not (Test-Path -LiteralPath $xmlSrc)) {
    throw "missing $xmlSrc"
}

if ([string]::IsNullOrWhiteSpace($WinswPath)) {
    $WinswPath = Join-Path $PackageRoot 'windows\winsw.exe'
}
if (-not (Test-Path -LiteralPath $WinswPath)) {
    throw @"
WinSW executable not found at $WinswPath.
Download a WinSW release binary, place it as windows\winsw.exe in the package,
then re-run. See packaging/windows/mpgs-server.xml header comments.
"@
}

$dataDir = Join-Path $PackageRoot 'data'
$logsDir = Join-Path $PackageRoot 'logs'
New-Item -ItemType Directory -Force -Path $dataDir, $logsDir | Out-Null

$serviceDir = Join-Path $PackageRoot 'windows'
$serviceExe = Join-Path $serviceDir ("{0}-service.exe" -f $ServiceName)
$serviceXml = Join-Path $serviceDir ("{0}-service.xml" -f $ServiceName)
Copy-Item -LiteralPath $WinswPath -Destination $serviceExe -Force
Copy-Item -LiteralPath $xmlSrc -Destination $serviceXml -Force

Write-Host "Installing service from $serviceExe (elevated rights required)..."
& $serviceExe install
if ($LASTEXITCODE -ne 0) { throw "winsw install failed: $LASTEXITCODE" }
& $serviceExe start
if ($LASTEXITCODE -ne 0) { throw "winsw start failed: $LASTEXITCODE" }
Write-Host "Service '$ServiceName' installed and started. Configure secrets in the service XML or host env before production use."
