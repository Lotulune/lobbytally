#Requires -Version 5.1
<#
.SYNOPSIS
  M6 release-hardening acceptance (offline; no signing secrets required).

.DESCRIPTION
  Verifies:
  - packaging layout files and ops/compliance docs exist
  - unit/integration gates for storage upgrade/backup and server m6 soak/fault
  - optional release package with PROVENANCE + SHA256SUMS
  - live meta provenance fields and backup/restore via dbtool
  - third-party license notice regenerates cleanly

.EXAMPLE
  .\scripts\m6_acceptance.ps1
  .\scripts\m6_acceptance.ps1 -Package
#>
param(
    [switch]$Package,
    [switch]$KeepArtifacts
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$results = [System.Collections.Generic.List[object]]::new()
$serverProc = $null
$workDir = $null
$fatalError = $null
$gitSha = (& git -C $repoRoot rev-parse HEAD 2>$null | Select-Object -First 1)
$gitDirty = [bool](& git -C $repoRoot status --porcelain 2>$null | Select-Object -First 1)
$scriptSha256 = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant()
$packagePath = ''

function Write-Step([string]$Name) {
    Write-Host ""
    Write-Host "==> $Name" -ForegroundColor Cyan
}

function Add-Result([string]$Id, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ id = $Id; ok = $Ok; detail = $Detail })
    if ($Ok) {
        Write-Host "  PASS  $Id — $Detail" -ForegroundColor Green
    } else {
        Write-Host "  FAIL  $Id — $Detail" -ForegroundColor Red
    }
}

function Test-RepoFile([string]$Rel) {
    return Test-Path -LiteralPath (Join-Path $repoRoot $Rel)
}

function Invoke-Json {
    param(
        [Parameter(Mandatory = $true)][string]$Method,
        [Parameter(Mandatory = $true)][string]$Url,
        [string]$Body = $null,
        [hashtable]$Headers = @{}
    )
    # Prefer curl.exe (shipped with Windows 10+) — more reliable than HttpClient /
    # HttpWebRequest under StrictMode + Start-Process redirected child servers.
    $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
    if ($null -eq $curl) {
        throw 'curl.exe is required for m6_acceptance HTTP probes'
    }
    $tmp = [System.IO.Path]::GetTempFileName()
    $bodyFile = $null
    try {
        $curlArgs = New-Object System.Collections.Generic.List[string]
        [void]$curlArgs.Add('--silent')
        [void]$curlArgs.Add('--show-error')
        [void]$curlArgs.Add('--max-time')
        [void]$curlArgs.Add('10')
        [void]$curlArgs.Add('-X')
        [void]$curlArgs.Add($Method.ToUpperInvariant())
        [void]$curlArgs.Add('-o')
        [void]$curlArgs.Add($tmp)
        [void]$curlArgs.Add('-w')
        [void]$curlArgs.Add('%{http_code}')
        foreach ($key in $Headers.Keys) {
            [void]$curlArgs.Add('-H')
            [void]$curlArgs.Add(('{0}: {1}' -f $key, [string]$Headers[$key]))
        }
        if ($null -ne $Body -and $Body.Length -gt 0) {
            $bodyFile = [System.IO.Path]::GetTempFileName()
            [System.IO.File]::WriteAllText($bodyFile, $Body, [System.Text.UTF8Encoding]::new($false))
            [void]$curlArgs.Add('-H')
            [void]$curlArgs.Add('Content-Type: application/json; charset=utf-8')
            [void]$curlArgs.Add('--data-binary')
            [void]$curlArgs.Add('@' + $bodyFile)
        }
        [void]$curlArgs.Add($Url)
        $prevEap = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $statusText = & curl.exe @($curlArgs.ToArray()) 2>$null
        $ErrorActionPreference = $prevEap
        $status = 0
        if ($statusText -match '^\d+$') {
            $status = [int]$statusText
        }
        $text = ''
        if (Test-Path -LiteralPath $tmp) {
            $text = [System.IO.File]::ReadAllText($tmp)
        }
        $json = $null
        if (-not [string]::IsNullOrWhiteSpace($text)) {
            try { $json = $text | ConvertFrom-Json } catch { $json = $null }
        }
        return [pscustomobject]@{
            StatusCode = $status
            Json       = $json
            Text       = $text
        }
    } finally {
        Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
        if ($null -ne $bodyFile) {
            Remove-Item -LiteralPath $bodyFile -Force -ErrorAction SilentlyContinue
        }
    }
}

function Write-Report([string]$Path, [bool]$Passed) {
    $lines = @(
        '# M6 acceptance run'
        ''
        "- When: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss K')"
        "- Result: $(if ($Passed) { 'PASS' } else { 'FAIL' })"
        "- Git commit: ``$gitSha``"
        "- Git worktree dirty: ``$gitDirty``"
        "- Acceptance script SHA-256: ``$scriptSha256``"
        "- Package built: ``$Package``"
        "- Package path: ``$packagePath``"
        "- Passed: $(($results | Where-Object ok).Count) / $($results.Count)"
        ''
        '| ID | OK | Detail |'
        '| --- | --- | --- |'
    )
    foreach ($r in $results) {
        $ok = if ($r.ok) { 'yes' } else { 'no' }
        $detail = ($r.detail -replace '\|', '/')
        $lines += "| $($r.id) | $ok | $detail |"
    }
    $lines += ''
    if ($Passed) {
        $lines += 'This run proves offline M6 release-hardening gates (docs, packaging layout, soak/fault/upgrade tests, meta provenance, backup/restore).'
    } else {
        $lines += 'This run does not close M6; inspect the failed checks above.'
    }
    $lines += 'Code signing, notarization, and production compliance signatures remain human gates (see SIGNING_AND_UPDATES.md / PRIVACY.md).'
    [System.IO.File]::WriteAllLines($Path, $lines)
}

try {
    Add-Result 'source.clean' (-not $gitDirty) "git_worktree_dirty=$gitDirty"

    Write-Step 'docs and packaging layout'
    $required = @(
        'docs/M6_ACCEPTANCE.md',
        'docs/OPERATIONS.md',
        'docs/ROLLBACK.md',
        'docs/KNOWN_LIMITATIONS.md',
        'docs/PRIVACY.md',
        'docs/SIGNING_AND_UPDATES.md',
        'docs/STEAM_BRAND_REVIEW.md',
        'docs/THIRD_PARTY_LICENSES.md',
        'packaging/common/mpgs.env.example',
        'packaging/linux/mpgs-server.service',
        'packaging/linux/install.sh',
        'packaging/windows/mpgs-server.xml',
        'packaging/windows/install-service.ps1',
        'packaging/windows/uninstall-service.ps1',
        'scripts/package_server.ps1',
        'scripts/generate_third_party_licenses.ps1',
        'scripts/m6_acceptance.ps1'
    )
    $missing = @($required | Where-Object { -not (Test-RepoFile $_) })
    Add-Result 'layout.files' ($missing.Count -eq 0) $(
        if ($missing.Count -eq 0) { "all $($required.Count) required paths present" }
        else { "missing: $($missing -join ', ')" }
    )

    Write-Step 'third-party license notice'
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repoRoot 'scripts\generate_third_party_licenses.ps1')
    if ($LASTEXITCODE -ne 0) { throw "generate_third_party_licenses failed: $LASTEXITCODE" }
    $lic = Join-Path $repoRoot 'docs\THIRD_PARTY_LICENSES.md'
    $licOk = (Test-Path -LiteralPath $lic) -and ((Get-Item -LiteralPath $lic).Length -gt 200)
    Add-Result 'licenses.generated' $licOk "THIRD_PARTY_LICENSES.md bytes=$((Get-Item $lic).Length)"

    Write-Step 'unit / integration gates'
    Push-Location $repoRoot
    try {
        cargo test -p mpgs-storage --locked --quiet -- m6_upgrade_path_from_each_intermediate_version
        if ($LASTEXITCODE -ne 0) { throw "storage upgrade test failed: $LASTEXITCODE" }
        cargo test -p mpgs-storage --locked --quiet -- backup_restore_and_integrity
        if ($LASTEXITCODE -ne 0) { throw "storage backup/restore test failed: $LASTEXITCODE" }
        Add-Result 'unit.storage_upgrade_backup' $true 'upgrade path + backup/restore tests passed'

        cargo test -p mpgs-server --locked --quiet -- m6_
        if ($LASTEXITCODE -ne 0) { throw "server m6 tests failed: $LASTEXITCODE" }
        Add-Result 'unit.server_m6' $true 'meta provenance + soak + fault tests passed'
    } finally {
        Pop-Location
    }

    Write-Step 'build tools'
    Push-Location $repoRoot
    try {
        $env:MPGS_BUILD_GIT_SHA = $gitSha
        cargo build -p mpgs-server -p mpgs-dbtool --locked --quiet
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed: $LASTEXITCODE" }
        Add-Result 'build.tools' $true "mpgs-server + mpgs-dbtool built sha=$gitSha"
    } finally {
        Pop-Location
    }

    $serverExe = Join-Path $repoRoot 'target\debug\mpgs-server.exe'
    if (-not (Test-Path $serverExe)) { $serverExe = Join-Path $repoRoot 'target\debug\mpgs-server' }
    $dbtool = Join-Path $repoRoot 'target\debug\mpgs-dbtool.exe'
    if (-not (Test-Path $dbtool)) { $dbtool = Join-Path $repoRoot 'target\debug\mpgs-dbtool' }

    $workDir = Join-Path $env:TEMP ("mpgs-m6-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $workDir -Force | Out-Null
    $dbPath = Join-Path $workDir 'm6.db'
    $backupPath = Join-Path $workDir 'm6-backup.db'
    $restorePath = Join-Path $workDir 'm6-restored.db'

    Write-Step 'server smoke + meta provenance'
    $port = Get-Random -Minimum 19000 -Maximum 20000
    $env:MPGS_DATABASE_PATH = $dbPath
    $env:MPGS_SEED_DEMO = 'true'
    $env:MPGS_AI_PROVIDER = 'disabled'
    $env:MPGS_AI_EMBED_PROVIDER = 'hash'
    $env:MPGS_BIND_ADDR = "127.0.0.1:$port"
    $serverLog = Join-Path $workDir 'server.out.log'
    $serverErr = Join-Path $workDir 'server.err.log'
    $serverProc = Start-Process -FilePath $serverExe -PassThru -WindowStyle Hidden `
        -WorkingDirectory $repoRoot `
        -RedirectStandardOutput $serverLog `
        -RedirectStandardError $serverErr
    $base = "http://127.0.0.1:$port"
    $ready = $false
    $readyDetail = "url=$base/health/ready"
    for ($i = 0; $i -lt 60; $i++) {
        [void]$serverProc.Refresh()
        if ($serverProc.HasExited) {
            $tail = ''
            if (Test-Path -LiteralPath $serverErr) {
                $tail = (Get-Content -LiteralPath $serverErr -Raw)
            }
            $readyDetail = "server exited early code=$($serverProc.ExitCode) err=$tail"
            break
        }
        try {
            $tcp = New-Object System.Net.Sockets.TcpClient
            $iar = $tcp.BeginConnect('127.0.0.1', [int]$port, $null, $null)
            $ok = $iar.AsyncWaitHandle.WaitOne(200)
            if ($ok -and $tcp.Connected) {
                $tcp.EndConnect($iar)
                $tcp.Close()
                $h = Invoke-Json -Method GET -Url "$base/health/live"
                if ([int]$h.StatusCode -eq 200) {
                    $r = Invoke-Json -Method GET -Url "$base/health/ready"
                    if ([int]$r.StatusCode -eq 200) {
                        $ready = $true
                        $readyDetail = "url=$base/health/ready live+ready=200"
                        break
                    }
                    $readyDetail = "ready_status=$($r.StatusCode) body=$($r.Text)"
                } else {
                    $readyDetail = "live_status=$($h.StatusCode) body=$($h.Text)"
                }
            } else {
                $tcp.Close()
                $readyDetail = "tcp not connected to 127.0.0.1:$port yet"
            }
        } catch {
            $readyDetail = "probe error: $($_.Exception.Message)"
        }
        Start-Sleep -Milliseconds 250
    }
    Add-Result 'runtime.ready' $ready $readyDetail

    if ($ready) {
        function Get-JsonProp([object]$Obj, [string]$Name) {
            if ($null -eq $Obj) { return $null }
            $prop = $Obj.PSObject.Properties[$Name]
            if ($null -eq $prop) { return $null }
            return $prop.Value
        }

        $meta = Invoke-Json -Method GET -Url "$base/v1/meta"
        $m = $meta.Json
        $metaOk = $meta.StatusCode -eq 200 -and
            (Get-JsonProp $m 'api_version') -eq 'v1' -and
            -not [string]::IsNullOrWhiteSpace([string](Get-JsonProp $m 'service_version')) -and
            -not [string]::IsNullOrWhiteSpace([string](Get-JsonProp $m 'algorithm_version')) -and
            $null -ne (Get-JsonProp $m 'schema_version') -and [int](Get-JsonProp $m 'schema_version') -gt 0 -and
            $null -ne (Get-JsonProp $m 'build_git_sha') -and
            $null -ne (Get-JsonProp $m 'data_updated_at_ms')
        Add-Result 'runtime.meta_provenance' $metaOk (
            "service=$(Get-JsonProp $m 'service_version') algo=$(Get-JsonProp $m 'algorithm_version') schema=$(Get-JsonProp $m 'schema_version') git=$(Get-JsonProp $m 'build_git_sha') data_ms=$(Get-JsonProp $m 'data_updated_at_ms')"
        )

        $feed = Invoke-Json -Method GET -Url "$base/v1/feeds/classic_legacy?limit=3"
        $items = Get-JsonProp $feed.Json 'items'
        $itemCount = if ($null -eq $items) { 0 } else { @($items).Count }
        $feedOk = $feed.StatusCode -eq 200 -and $itemCount -gt 0
        Add-Result 'runtime.feed' $feedOk "status=$($feed.StatusCode) items=$itemCount"

        $nl = Invoke-Json -Method POST -Url "$base/v1/recommendations/natural-language" `
            -Body '{"query":"4 coop self-host","limit":3}'
        $aiStatus = [string](Get-JsonProp $nl.Json 'ai_status')
        $nlOk = $nl.StatusCode -eq 200 -and $aiStatus -eq 'fallback'
        Add-Result 'runtime.nl_fallback' $nlOk "status=$($nl.StatusCode) ai_status=$aiStatus body=$($nl.Text.Substring(0, [Math]::Min(120, $nl.Text.Length)))"
    } else {
        Add-Result 'runtime.meta_provenance' $false 'server never became ready'
        Add-Result 'runtime.feed' $false 'skipped'
        Add-Result 'runtime.nl_fallback' $false 'skipped'
    }

    Write-Step 'backup / restore'
    if ($serverProc -and -not $serverProc.HasExited) {
        Stop-Process -Id $serverProc.Id -Force -ErrorAction SilentlyContinue
        $serverProc = $null
        Start-Sleep -Milliseconds 400
    }
    & $dbtool backup $dbPath $backupPath
    if ($LASTEXITCODE -ne 0) { throw "dbtool backup failed: $LASTEXITCODE" }
    & $dbtool restore $backupPath $restorePath
    if ($LASTEXITCODE -ne 0) { throw "dbtool restore failed: $LASTEXITCODE" }
    & $dbtool integrity $restorePath
    if ($LASTEXITCODE -ne 0) { throw "dbtool integrity failed: $LASTEXITCODE" }
    Add-Result 'ops.backup_restore' $true "backup+restore+integrity ok"

    if ($Package) {
        Write-Step 'release package layout'
        $pkgOut = Join-Path $workDir 'dist'
        Push-Location $repoRoot
        try {
            $env:MPGS_BUILD_GIT_SHA = $gitSha
            cargo build -p mpgs-server -p mpgs-dbtool --release --locked --quiet
            if ($LASTEXITCODE -ne 0) { throw "release build failed: $LASTEXITCODE" }
        } finally {
            Pop-Location
        }
        & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repoRoot 'scripts\package_server.ps1') -OutDir $pkgOut -SkipBuild
        if ($LASTEXITCODE -ne 0) { throw "package_server failed: $LASTEXITCODE" }
        $pkgDir = Get-ChildItem -LiteralPath $pkgOut -Directory -ErrorAction SilentlyContinue | Select-Object -First 1
        $packagePath = if ($pkgDir) { $pkgDir.FullName } else { '' }
        $prov = Join-Path $packagePath 'PROVENANCE.json'
        $sums = Join-Path $packagePath 'SHA256SUMS.txt'
        $serverName = if ($env:OS -eq 'Windows_NT') { 'mpgs-server.exe' } else { 'mpgs-server' }
        $binOk = $packagePath -and (Test-Path (Join-Path $packagePath "bin\$serverName"))
        $pkgOk = $binOk -and (Test-Path $prov) -and (Test-Path $sums)
        if ($pkgOk) {
            $provObj = Get-Content -LiteralPath $prov -Raw | ConvertFrom-Json
            $pkgOk = $provObj.signing -eq 'unsigned' -and [int]$provObj.schema_version -gt 0
        }
        Add-Result 'package.provenance' ([bool]$pkgOk) "path=$packagePath"
    } else {
        Add-Result 'package.provenance' $true 'skipped (-Package not set); layout scripts present'
    }

    $failed = @($results | Where-Object { -not $_.ok })
    # Dirty tree fails source.clean but we still write report; overall pass requires all ok.
    $passed = $failed.Count -eq 0
    Write-Report (Join-Path $repoRoot 'docs\M6_ACCEPTANCE_RUN.md') $passed
    if (-not $passed) {
        throw "M6 acceptance failed: $($failed.Count) check(s)"
    }
    Write-Host ""
    Write-Host "M6 acceptance PASS ($($results.Count)/$($results.Count))" -ForegroundColor Green
}
catch {
    $fatalError = $_
    try {
        Write-Report (Join-Path $repoRoot 'docs\M6_ACCEPTANCE_RUN.md') $false
    } catch { }
    Write-Host "M6 acceptance FATAL: $fatalError" -ForegroundColor Red
    exit 1
}
finally {
    if ($serverProc -and -not $serverProc.HasExited) {
        Stop-Process -Id $serverProc.Id -Force -ErrorAction SilentlyContinue
    }
    foreach ($key in @('MPGS_DATABASE_PATH', 'MPGS_SEED_DEMO', 'MPGS_AI_PROVIDER', 'MPGS_BIND_ADDR')) {
        Remove-Item "Env:$key" -ErrorAction SilentlyContinue
    }
    if ($workDir -and (Test-Path -LiteralPath $workDir)) {
        if ($KeepArtifacts -or $null -ne $fatalError) {
            Write-Host "artifacts kept: $workDir" -ForegroundColor Yellow
        } else {
            Remove-Item -LiteralPath $workDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
