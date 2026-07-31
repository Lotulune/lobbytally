#Requires -Version 5.1
<#
.SYNOPSIS
  Install MPGS as a low-privilege Windows service via WinSW.

.DESCRIPTION
  Copies executable content into a protected Program Files directory, keeps
  mutable data under ProgramData, grants only LocalService write access to the
  data tree, and installs a Manual service by default. Pass -Start together
  with -AdminToken to install it as Automatic and start it.

.PARAMETER PackageRoot
  Extracted release package produced by scripts/package_server.ps1.

.PARAMETER WinswPath
  WinSW executable. Defaults to PackageRoot\windows\winsw.exe.

.PARAMETER InstallRoot
  Protected executable root. Defaults to %ProgramFiles%\MPGS.

.PARAMETER DataRoot
  Protected mutable-data root. Defaults to %ProgramData%\MPGS.

.PARAMETER AdminToken
  Secure admin token written only to the ACL-protected installed XML.

.PARAMETER Start
  Start after installation. Requires a non-empty AdminToken.
#>
param(
    [Parameter(Mandatory = $true)][string]$PackageRoot,
    [string]$WinswPath = '',
    [string]$ServiceName = 'mpgs-server',
    [string]$InstallRoot = '',
    [string]$DataRoot = '',
    [Security.SecureString]$AdminToken,
    [switch]$Start
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'install-service.ps1 must run from an elevated PowerShell session'
    }
}

function Resolve-SafeDirectoryPath([string]$Path, [string]$Name) {
    $full = [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($Path))
    $root = [IO.Path]::GetPathRoot($full)
    if ($full.TrimEnd('\', '/') -eq $root.TrimEnd('\', '/')) {
        throw "$Name must not be a filesystem root: $full"
    }
    return $full
}

function Assert-StrictChildPath([string]$Path, [string]$Parent, [string]$Name) {
    $parentFull = [IO.Path]::GetFullPath($Parent).TrimEnd('\', '/')
    $prefix = $parentFull + [IO.Path]::DirectorySeparatorChar
    if (-not $Path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "$Name must be a child directory of $parentFull (got: $Path)"
    }
}

function ConvertFrom-SecureStringPlain([Security.SecureString]$Value) {
    if ($null -eq $Value) { return '' }
    $pointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($Value)
    try {
        return [Runtime.InteropServices.Marshal]::PtrToStringBSTR($pointer)
    } finally {
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($pointer)
    }
}

function Set-ProtectedDirectoryAcl(
    [string]$Path,
    [Security.AccessControl.FileSystemRights]$ServiceRights
) {
    $inheritance = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
    $propagation = [Security.AccessControl.PropagationFlags]::None
    $allow = [Security.AccessControl.AccessControlType]::Allow
    $security = New-Object Security.AccessControl.DirectorySecurity
    $security.SetAccessRuleProtection($true, $false)

    $rules = @(
        @('S-1-5-18', [Security.AccessControl.FileSystemRights]::FullControl),
        @('S-1-5-32-544', [Security.AccessControl.FileSystemRights]::FullControl),
        @('S-1-5-19', $ServiceRights)
    )
    foreach ($rule in $rules) {
        $sid = New-Object Security.Principal.SecurityIdentifier($rule[0])
        $accessRule = New-Object Security.AccessControl.FileSystemAccessRule(
            $sid,
            $rule[1],
            $inheritance,
            $propagation,
            $allow
        )
        [void]$security.AddAccessRule($accessRule)
    }
    $owner = New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')
    $security.SetOwner($owner)
    Set-Acl -LiteralPath $Path -AclObject $security
}

function Copy-IfDifferent([string]$Source, [string]$Destination) {
    $sourceFull = [IO.Path]::GetFullPath($Source)
    $destinationFull = [IO.Path]::GetFullPath($Destination)
    if (-not $sourceFull.Equals($destinationFull, [StringComparison]::OrdinalIgnoreCase)) {
        Copy-Item -LiteralPath $sourceFull -Destination $destinationFull -Force
    }
}

function Require-XmlNode($Node, [string]$Description) {
    if ($null -eq $Node) { throw "service XML is missing $Description" }
    return $Node
}

Assert-Administrator
if ($ServiceName -notmatch '^[A-Za-z0-9][A-Za-z0-9_.-]{0,79}$') {
    throw 'ServiceName must contain only letters, digits, dot, underscore, or hyphen'
}

$PackageRoot = (Resolve-Path -LiteralPath $PackageRoot).Path
if ([string]::IsNullOrWhiteSpace($InstallRoot)) {
    $InstallRoot = Join-Path $env:ProgramFiles 'MPGS'
}
if ([string]::IsNullOrWhiteSpace($DataRoot)) {
    $DataRoot = Join-Path $env:ProgramData 'MPGS'
}
$InstallRoot = Resolve-SafeDirectoryPath $InstallRoot 'InstallRoot'
$DataRoot = Resolve-SafeDirectoryPath $DataRoot 'DataRoot'
Assert-StrictChildPath $InstallRoot $env:ProgramFiles 'InstallRoot'
Assert-StrictChildPath $DataRoot $env:ProgramData 'DataRoot'

$serverSrc = Join-Path $PackageRoot 'bin\mpgs-server.exe'
$dbtoolSrc = Join-Path $PackageRoot 'bin\mpgs-dbtool.exe'
$xmlSrc = Join-Path $PackageRoot 'windows\mpgs-server.xml'
if (-not (Test-Path -LiteralPath $serverSrc -PathType Leaf)) {
    throw "missing $serverSrc — build a package with scripts/package_server.ps1 first"
}
if (-not (Test-Path -LiteralPath $xmlSrc -PathType Leaf)) {
    throw "missing $xmlSrc"
}
if ([string]::IsNullOrWhiteSpace($WinswPath)) {
    $WinswPath = Join-Path $PackageRoot 'windows\winsw.exe'
}
$WinswPath = (Resolve-Path -LiteralPath $WinswPath).Path

if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
    throw "service '$ServiceName' already exists; uninstall it before replacing its executable root"
}

$plainAdminToken = ConvertFrom-SecureStringPlain $AdminToken
if ($Start -and [string]::IsNullOrWhiteSpace($plainAdminToken)) {
    throw '-Start requires a non-empty -AdminToken SecureString'
}

New-Item -ItemType Directory -Force -Path $InstallRoot, $DataRoot | Out-Null
Set-ProtectedDirectoryAcl $InstallRoot ([Security.AccessControl.FileSystemRights]::ReadAndExecute)
Set-ProtectedDirectoryAcl $DataRoot ([Security.AccessControl.FileSystemRights]::Modify)

$binDir = Join-Path $InstallRoot 'bin'
$serviceDir = Join-Path $InstallRoot 'windows'
$dataDir = Join-Path $DataRoot 'data'
$logsDir = Join-Path $DataRoot 'logs'
New-Item -ItemType Directory -Force -Path $binDir, $serviceDir, $dataDir, $logsDir | Out-Null

Copy-IfDifferent $serverSrc (Join-Path $binDir 'mpgs-server.exe')
if (Test-Path -LiteralPath $dbtoolSrc -PathType Leaf) {
    Copy-IfDifferent $dbtoolSrc (Join-Path $binDir 'mpgs-dbtool.exe')
}
$uninstallSrc = Join-Path $PackageRoot 'windows\uninstall-service.ps1'
if (Test-Path -LiteralPath $uninstallSrc -PathType Leaf) {
    Copy-IfDifferent $uninstallSrc (Join-Path $serviceDir 'uninstall-service.ps1')
}

$serviceExe = Join-Path $serviceDir ("{0}-service.exe" -f $ServiceName)
$serviceXml = Join-Path $serviceDir ("{0}-service.xml" -f $ServiceName)
Copy-IfDifferent $WinswPath $serviceExe

[xml]$config = Get-Content -LiteralPath $xmlSrc -Raw
$service = Require-XmlNode $config.service '<service>'
(Require-XmlNode $service.id '<id>').InnerText = $ServiceName
(Require-XmlNode $service.name '<name>').InnerText = 'LobbyTally Server'
(Require-XmlNode $service.workingdirectory '<workingdirectory>').InnerText = $DataRoot
(Require-XmlNode $service.logpath '<logpath>').InnerText = $logsDir
(Require-XmlNode $service.startmode '<startmode>').InnerText = if ($Start) { 'Automatic' } else { 'Manual' }

$databaseEnv = @($service.env | Where-Object { $_.name -eq 'MPGS_DATABASE_PATH' }) | Select-Object -First 1
$adminEnv = @($service.env | Where-Object { $_.name -eq 'MPGS_ADMIN_TOKEN' }) | Select-Object -First 1
(Require-XmlNode $databaseEnv 'MPGS_DATABASE_PATH env').SetAttribute(
    'value',
    (Join-Path $dataDir 'mpgs.db')
)
(Require-XmlNode $adminEnv 'MPGS_ADMIN_TOKEN env').SetAttribute('value', $plainAdminToken)
$config.Save($serviceXml)
$plainAdminToken = $null

# Re-apply the root ACLs after copying so every new child inherits the protected rules.
Set-ProtectedDirectoryAcl $InstallRoot ([Security.AccessControl.FileSystemRights]::ReadAndExecute)
Set-ProtectedDirectoryAcl $DataRoot ([Security.AccessControl.FileSystemRights]::Modify)

Write-Host "Installing service from protected root $serviceExe ..."
& $serviceExe install
if ($LASTEXITCODE -ne 0) { throw "winsw install failed: $LASTEXITCODE" }

if ($Start) {
    & $serviceExe start
    if ($LASTEXITCODE -ne 0) { throw "winsw start failed: $LASTEXITCODE" }
    Write-Host "Service '$ServiceName' installed and started as LocalService."
} else {
    Write-Host "Service '$ServiceName' installed but not started."
    Write-Host "Review the protected XML, then start it with: & '$serviceExe' start"
}
Write-Host "Executables: $InstallRoot"
Write-Host "Data/logs:  $DataRoot"
