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

  Stamps MPGS_BUILD_GIT_SHA into the binaries when Git is available.
  Does not sign artifacts (see docs/SIGNING_AND_UPDATES.md).

.PARAMETER OutDir
  Output directory (default: dist).

.PARAMETER Target
  Optional rustc target triple. Empty = host.

.PARAMETER SkipBuild
  Package already-built release binaries from target/[triple/]release.
#>
param(
    [string]$OutDir = 'dist',
    [string]$Target = '',
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
Push-Location $repoRoot
try {
    $version = '0.1.0'
    $cargoToml = Get-Content -LiteralPath (Join-Path $repoRoot 'Cargo.toml') -Raw
    if ($cargoToml -match 'version\s*=\s*"([^"]+)"') {
        $version = $Matches[1]
    }

    $gitSha = (& git rev-parse HEAD 2>$null | Select-Object -First 1)
    if ([string]::IsNullOrWhiteSpace($gitSha)) { $gitSha = 'unknown' }
    $gitShort = if ($gitSha.Length -ge 7) { $gitSha.Substring(0, 7) } else { $gitSha }
    $builtAt = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')

    $env:MPGS_BUILD_GIT_SHA = $gitSha

    if (-not $SkipBuild) {
        $buildArgs = @('build', '-p', 'mpgs-server', '-p', 'mpgs-dbtool', '--release', '--locked')
        if (-not [string]::IsNullOrWhiteSpace($Target)) {
            $buildArgs += @('--target', $Target)
        }
        Write-Host "==> cargo $($buildArgs -join ' ')"
        & cargo @buildArgs
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed: $LASTEXITCODE" }
    }

    $releaseDir = if ([string]::IsNullOrWhiteSpace($Target)) {
        Join-Path $repoRoot 'target\release'
    } else {
        Join-Path $repoRoot ("target\{0}\release" -f $Target)
    }

    $isWindows = $env:OS -eq 'Windows_NT'
    $exe = if ($isWindows) { '.exe' } else { '' }
    $serverSrc = Join-Path $releaseDir ("mpgs-server{0}" -f $exe)
    $dbtoolSrc = Join-Path $releaseDir ("mpgs-dbtool{0}" -f $exe)
    if (-not (Test-Path -LiteralPath $serverSrc)) { throw "missing $serverSrc" }
    if (-not (Test-Path -LiteralPath $dbtoolSrc)) { throw "missing $dbtoolSrc" }

    $osName = if ($isWindows) { 'windows' } else { 'linux' }
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
    if ($arch -eq 'x64') { $arch = 'x64' }
    elseif ($arch -eq 'arm64') { $arch = 'arm64' }

    $pkgName = "mpgs-server-$osName-$arch-$version+$gitShort"
    $outRoot = if ([System.IO.Path]::IsPathRooted($OutDir)) {
        $OutDir
    } else {
        Join-Path $repoRoot $OutDir
    }
    New-Item -ItemType Directory -Force -Path $outRoot | Out-Null
    $pkgRoot = Join-Path $outRoot $pkgName
    if (Test-Path -LiteralPath $pkgRoot) {
        Remove-Item -LiteralPath $pkgRoot -Recurse -Force
    }
    $binDir = Join-Path $pkgRoot 'bin'
    New-Item -ItemType Directory -Force -Path $binDir | Out-Null
    Copy-Item -LiteralPath $serverSrc -Destination (Join-Path $binDir ("mpgs-server{0}" -f $exe))
    Copy-Item -LiteralPath $dbtoolSrc -Destination (Join-Path $binDir ("mpgs-dbtool{0}" -f $exe))

    # Packaging assets
    $packaging = Join-Path $repoRoot 'packaging'
    Copy-Item -LiteralPath (Join-Path $packaging 'common') -Destination (Join-Path $pkgRoot 'common') -Recurse
    Copy-Item -LiteralPath (Join-Path $packaging 'linux') -Destination (Join-Path $pkgRoot 'linux') -Recurse
    Copy-Item -LiteralPath (Join-Path $packaging 'windows') -Destination (Join-Path $pkgRoot 'windows') -Recurse

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
        $src = Join-Path $repoRoot "docs\$doc"
        if (Test-Path -LiteralPath $src) {
            Copy-Item -LiteralPath $src -Destination (Join-Path $docsOut $doc)
        }
    }

    $schemaVersion = 7
    $algorithmVersion = 'rules-0.2.0'
    $provenance = [ordered]@{
        product             = 'mpgs-server'
        service_version     = $version
        git_sha             = $gitSha
        built_at_utc        = $builtAt
        rustc_target        = if ($Target) { $Target } else { 'host' }
        schema_version      = $schemaVersion
        algorithm_version   = $algorithmVersion
        signing             = 'unsigned'
        package_layout      = 'm6-server-1'
    }
    $provenancePath = Join-Path $pkgRoot 'PROVENANCE.json'
    ($provenance | ConvertTo-Json -Depth 4) | Set-Content -LiteralPath $provenancePath -Encoding UTF8

    $sums = New-Object System.Collections.Generic.List[string]
    Get-ChildItem -LiteralPath $pkgRoot -Recurse -File | ForEach-Object {
        $rel = $_.FullName.Substring($pkgRoot.Length).TrimStart('\', '/')
        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        $sums.Add("$hash  $rel")
    }
    $sumsPath = Join-Path $pkgRoot 'SHA256SUMS.txt'
    [System.IO.File]::WriteAllLines($sumsPath, $sums)

    Write-Host "Package ready: $pkgRoot"
    Write-Host "Provenance: service=$version git=$gitShort schema=$schemaVersion algorithm=$algorithmVersion unsigned"
    Write-Output $pkgRoot
}
finally {
    Pop-Location
}
