#Requires -Version 5.1
<#
.SYNOPSIS
  Build a traceable MPGS server/dbtool package layout for Windows or Linux hosts.

.DESCRIPTION
  Produces:
    dist/mpgs-server-<os>-<arch>-<version>/
      bin/mpgs-server[.exe]
      bin/mpgs-dbtool[.exe]
      common/mpgs.env.example
      linux/... or windows/...
      docs/ (ops subset)
      PROVENANCE.json
      SHA256SUMS.txt

  Stamps MPGS_BUILD_GIT_SHA into the server and validates native binaries against
  their compiled --build-info before writing provenance. Does not sign artifacts.

.PARAMETER OutDir
  Output directory (default: dist).

.PARAMETER Target
  Optional Rust target triple. Empty uses the rustc host target.

.PARAMETER SkipBuild
  Package already-built release binaries. This is allowed only for the native
  target because the binary must be executed to verify its compiled build info.
#>
param(
    [string]$OutDir = 'dist',
    [string]$Target = '',
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-HostTarget {
    $lines = & rustc -vV
    if ($LASTEXITCODE -ne 0) { throw "rustc -vV failed: $LASTEXITCODE" }
    foreach ($line in $lines) {
        if ([string]$line -match '^host:\s*(\S+)$') {
            return [string]$Matches[1]
        }
    }
    throw 'could not determine rustc host target'
}

function Resolve-PackagePlatform([string]$RustTarget) {
    $arch = if ($RustTarget -match '^x86_64-') {
        'x64'
    } elseif ($RustTarget -match '^aarch64-') {
        'arm64'
    } else {
        throw "unsupported package architecture in target: $RustTarget"
    }

    if ($RustTarget -match '-windows-') {
        return [pscustomobject]@{ os = 'windows'; arch = $arch; suffix = '.exe' }
    }
    if ($RustTarget -match '-linux-') {
        return [pscustomobject]@{ os = 'linux'; arch = $arch; suffix = '' }
    }
    throw "unsupported package operating system in target: $RustTarget"
}

function Read-NativeBuildInfo([string]$ServerPath) {
    $output = & $ServerPath --build-info
    if ($LASTEXITCODE -ne 0) { throw "server --build-info failed: $LASTEXITCODE" }
    $jsonText = if ($output -is [array]) { $output -join "`n" } else { [string]$output }
    try {
        return $jsonText | ConvertFrom-Json
    } catch {
        throw "server --build-info returned invalid JSON: $jsonText"
    }
}

function Read-SourceBuildInfo([string]$Root, [string]$Version, [string]$GitSha, [string]$RustTarget) {
    $migrationVersions = @(
        Get-ChildItem -LiteralPath (Join-Path $Root 'migrations') -File -Filter '*.sql' |
            ForEach-Object {
                if ($_.BaseName -match '^(\d{4})_') { [int]$Matches[1] }
            }
    )
    if ($migrationVersions.Count -eq 0) { throw 'no numbered migrations found' }
    $schemaVersion = ($migrationVersions | Measure-Object -Maximum).Maximum

    $recommender = Get-Content -LiteralPath (Join-Path $Root 'crates/recommender/src/lib.rs') -Raw
    if ($recommender -notmatch 'pub const ALGORITHM_VERSION:\s*&str\s*=\s*"([^"]+)"') {
        throw 'could not read ALGORITHM_VERSION from recommender'
    }
    return [pscustomobject]@{
        product           = 'mpgs-server'
        service_version   = $Version
        git_sha           = $GitSha
        rustc_target      = $RustTarget
        schema_version    = [int]$schemaVersion
        algorithm_version = [string]$Matches[1]
    }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$previousBuildGitSha = [Environment]::GetEnvironmentVariable('MPGS_BUILD_GIT_SHA', 'Process')
Push-Location $repoRoot
try {
    $cargoToml = Get-Content -LiteralPath (Join-Path $repoRoot 'Cargo.toml') -Raw
    if ($cargoToml -notmatch '(?m)^version\s*=\s*"([^"]+)"\s*$') {
        throw 'workspace package version not found in Cargo.toml'
    }
    $version = [string]$Matches[1]

    $gitOutput = & git rev-parse HEAD 2>$null
    $gitCode = $LASTEXITCODE
    $gitSha = [string]($gitOutput | Select-Object -First 1)
    if ($gitCode -ne 0 -or $gitSha -notmatch '^[0-9a-fA-F]{40}$') {
        throw 'a full Git commit SHA is required to build a release package'
    }
    $gitSha = $gitSha.ToLowerInvariant()
    $gitShort = $gitSha.Substring(0, 7)
    $sourceDirty = [bool](& git status --porcelain 2>$null | Select-Object -First 1)
    $builtAt = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')

    $hostTarget = Get-HostTarget
    $resolvedTarget = if ([string]::IsNullOrWhiteSpace($Target)) { $hostTarget } else { $Target.Trim() }
    $platform = Resolve-PackagePlatform $resolvedTarget
    if ($SkipBuild -and $resolvedTarget -ne $hostTarget) {
        throw "-SkipBuild requires native target $hostTarget; cannot verify $resolvedTarget on this host"
    }

    $env:MPGS_BUILD_GIT_SHA = $gitSha
    if (-not $SkipBuild) {
        $buildArgs = @('build', '-p', 'mpgs-server', '-p', 'mpgs-dbtool', '--release', '--locked')
        if (-not [string]::IsNullOrWhiteSpace($Target)) {
            $buildArgs += @('--target', $resolvedTarget)
        }
        Write-Host "==> cargo $($buildArgs -join ' ')"
        & cargo @buildArgs
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed: $LASTEXITCODE" }
    }

    $releaseDir = if ([string]::IsNullOrWhiteSpace($Target)) {
        Join-Path $repoRoot 'target/release'
    } else {
        Join-Path $repoRoot ("target/{0}/release" -f $resolvedTarget)
    }
    $serverSrc = Join-Path $releaseDir ("mpgs-server{0}" -f $platform.suffix)
    $dbtoolSrc = Join-Path $releaseDir ("mpgs-dbtool{0}" -f $platform.suffix)
    if (-not (Test-Path -LiteralPath $serverSrc -PathType Leaf)) { throw "missing $serverSrc" }
    if (-not (Test-Path -LiteralPath $dbtoolSrc -PathType Leaf)) { throw "missing $dbtoolSrc" }

    $buildInfo = if ($resolvedTarget -eq $hostTarget) {
        Read-NativeBuildInfo $serverSrc
    } else {
        Read-SourceBuildInfo $repoRoot $version $gitSha $resolvedTarget
    }
    $expected = @{
        product         = 'mpgs-server'
        service_version = $version
        git_sha         = $gitSha
        rustc_target    = $resolvedTarget
    }
    foreach ($key in $expected.Keys) {
        if ([string]$buildInfo.$key -ne [string]$expected[$key]) {
            throw "compiled build info mismatch for ${key}: expected=$($expected[$key]) actual=$($buildInfo.$key)"
        }
    }
    if ([int]$buildInfo.schema_version -le 0) { throw 'compiled schema_version must be positive' }
    if ([string]::IsNullOrWhiteSpace([string]$buildInfo.algorithm_version)) {
        throw 'compiled algorithm_version must not be empty'
    }

    $pkgName = "mpgs-server-$($platform.os)-$($platform.arch)-$version+$gitShort"
    $outRoot = if ([System.IO.Path]::IsPathRooted($OutDir)) { $OutDir } else { Join-Path $repoRoot $OutDir }
    New-Item -ItemType Directory -Force -Path $outRoot | Out-Null
    $pkgRoot = Join-Path $outRoot $pkgName
    if (Test-Path -LiteralPath $pkgRoot) {
        Remove-Item -LiteralPath $pkgRoot -Recurse -Force
    }

    New-Item -ItemType Directory -Force -Path $pkgRoot | Out-Null
    $pkgRoot = (Get-Item -LiteralPath $pkgRoot).FullName
    $binDir = Join-Path $pkgRoot 'bin'
    New-Item -ItemType Directory -Force -Path $binDir | Out-Null
    Copy-Item -LiteralPath $serverSrc -Destination (Join-Path $binDir ("mpgs-server{0}" -f $platform.suffix))
    Copy-Item -LiteralPath $dbtoolSrc -Destination (Join-Path $binDir ("mpgs-dbtool{0}" -f $platform.suffix))

    $packaging = Join-Path $repoRoot 'packaging'
    Copy-Item -LiteralPath (Join-Path $packaging 'common') -Destination (Join-Path $pkgRoot 'common') -Recurse
    Copy-Item -LiteralPath (Join-Path $packaging $platform.os) -Destination (Join-Path $pkgRoot $platform.os) -Recurse
    if ($platform.os -eq 'linux' -and $env:OS -ne 'Windows_NT') {
        & chmod 0755 (Join-Path $pkgRoot 'linux/install.sh')
        if ($LASTEXITCODE -ne 0) { throw "chmod install.sh failed: $LASTEXITCODE" }
    }

    if ($platform.os -eq 'windows') {
        [xml]$serviceXml = Get-Content -LiteralPath (Join-Path $pkgRoot 'windows/mpgs-server.xml') -Raw
        if ([string]$serviceXml.service.executable -ne '%BASE%\..\bin\mpgs-server.exe') {
            throw 'WinSW executable path must resolve from windows/ to bin/mpgs-server.exe'
        }
        if ([string]$serviceXml.service.serviceaccount.username -ne 'NT AUTHORITY\LocalService') {
            throw 'WinSW service template must use the low-privilege LocalService account'
        }
        if ([string]$serviceXml.service.startmode -ne 'Manual') {
            throw 'WinSW service template must remain Manual until the installer receives an admin token'
        }
        $adminTokenEnv = @(
            $serviceXml.service.env | Where-Object { $_.name -eq 'MPGS_ADMIN_TOKEN' }
        ) | Select-Object -First 1
        if ($null -eq $adminTokenEnv -or -not [string]::IsNullOrEmpty([string]$adminTokenEnv.value)) {
            throw 'WinSW service template must not contain an MPGS_ADMIN_TOKEN'
        }
    }

    $docsOut = Join-Path $pkgRoot 'docs'
    New-Item -ItemType Directory -Force -Path $docsOut | Out-Null
    foreach ($doc in @(
            'OPERATIONS.md',
            'ROLLBACK.md',
            'KNOWN_LIMITATIONS.md',
            'PRIVACY.md',
            'SIGNING_AND_UPDATES.md',
            'THIRD_PARTY_LICENSES.md',
            'STEAM_BRAND_REVIEW.md'
        )) {
        $src = Join-Path $repoRoot "docs/$doc"
        if (-not (Test-Path -LiteralPath $src -PathType Leaf)) { throw "missing release document: $src" }
        Copy-Item -LiteralPath $src -Destination (Join-Path $docsOut $doc)
    }

    $provenance = [ordered]@{
        product             = [string]$buildInfo.product
        service_version     = [string]$buildInfo.service_version
        git_sha             = [string]$buildInfo.git_sha
        source_dirty        = $sourceDirty
        built_at_utc        = $builtAt
        rustc_target        = [string]$buildInfo.rustc_target
        schema_version      = [int]$buildInfo.schema_version
        algorithm_version   = [string]$buildInfo.algorithm_version
        signing             = 'unsigned'
        package_layout      = 'm6-server-2'
    }
    $provenancePath = Join-Path $pkgRoot 'PROVENANCE.json'
    ($provenance | ConvertTo-Json -Depth 4) | Set-Content -LiteralPath $provenancePath -Encoding UTF8

    $sums = [System.Collections.Generic.List[string]]::new()
    Get-ChildItem -LiteralPath $pkgRoot -Recurse -File | Sort-Object FullName | ForEach-Object {
        $rel = $_.FullName.Substring($pkgRoot.Length).TrimStart('\', '/')
        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        $sums.Add("$hash  $rel")
    }
    $sumsPath = Join-Path $pkgRoot 'SHA256SUMS.txt'
    [System.IO.File]::WriteAllLines($sumsPath, $sums)

    Write-Host "Package ready: $pkgRoot"
    Write-Host "Provenance: service=$version git=$gitShort target=$resolvedTarget schema=$($buildInfo.schema_version) algorithm=$($buildInfo.algorithm_version) dirty=$sourceDirty unsigned"
    Write-Output $pkgRoot
}
finally {
    if ($null -eq $previousBuildGitSha) {
        Remove-Item Env:MPGS_BUILD_GIT_SHA -ErrorAction SilentlyContinue
    } else {
        $env:MPGS_BUILD_GIT_SHA = $previousBuildGitSha
    }
    Pop-Location
}
