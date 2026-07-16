#Requires -Version 5.1
param(
    [Parameter(Mandatory = $true)][string]$PackageRoot,
    [string]$ServiceName = 'mpgs-server'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$PackageRoot = (Resolve-Path -LiteralPath $PackageRoot).Path
$serviceExe = Join-Path $PackageRoot ("windows\{0}-service.exe" -f $ServiceName)
if (-not (Test-Path -LiteralPath $serviceExe)) {
    throw "service wrapper not found: $serviceExe"
}

& $serviceExe stop
& $serviceExe uninstall
Write-Host "Service '$ServiceName' stopped and uninstalled."
