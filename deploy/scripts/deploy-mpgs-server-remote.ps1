param(
    [Parameter(Mandatory = $true)]
    [string] $RemoteHost,

    [string] $RemotePath = "~/mpgs-server",
    [string] $ImageTar = "mpgs-server-local.tar",
    [string] $ProjectName = "mpgs",
    [switch] $UseCaddy,
    [switch] $UseSudoDocker,
    [string] $PublicBaseUrl = ""
)

$ErrorActionPreference = "Stop"

function Require-Command {
    param([string] $Name)

    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "$Name is required on the local deploy machine."
    }
}

function Invoke-Checked {
    param(
        [string] $CommandName,
        [string[]] $Arguments
    )

    & $CommandName @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$CommandName failed with exit code $LASTEXITCODE."
    }
}

function Format-RemoteTarget {
    param(
        [string] $Host,
        [string] $Path
    )

    return "${Host}:$Path"
}

$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$imageTarPath = if ([System.IO.Path]::IsPathRooted($ImageTar)) {
    $ImageTar
} else {
    Join-Path $root $ImageTar
}

if (-not (Test-Path $imageTarPath)) {
    throw "Image archive not found: $imageTarPath. Run deploy/scripts/build-mpgs-server-image.ps1 first."
}

Require-Command "ssh"
Require-Command "scp"

if ($RemotePath -match "\s" -or $RemotePath -match "'") {
    throw "RemotePath must not contain whitespace or single quotes."
}
if ($RemotePath -notmatch "^[A-Za-z0-9_./~:-]+$") {
    throw "RemotePath contains unsupported shell characters."
}
if ($ProjectName -notmatch "^[A-Za-z0-9_.-]+$") {
    throw "ProjectName must contain only letters, numbers, dot, underscore, or hyphen."
}

$remoteImageName = Split-Path $imageTarPath -Leaf
if ($remoteImageName -notmatch "^[A-Za-z0-9_.-]+$") {
    throw "Image archive filename must contain only letters, numbers, dot, underscore, or hyphen."
}

Invoke-Checked "ssh" @($RemoteHost, "mkdir -p $RemotePath/deploy/config/active $RemotePath/deploy/config/pending")

$deployFiles = @(
    "deploy/compose.yml",
    "deploy/compose.caddy.yml",
    "deploy/Caddyfile",
    "deploy/mpgs-server.env.example"
)

foreach ($file in $deployFiles) {
    Invoke-Checked "scp" @((Join-Path $root $file), (Format-RemoteTarget $RemoteHost "$RemotePath/deploy/"))
}

Invoke-Checked "scp" @((Join-Path $root "deploy/config/setup.toml.example"), (Format-RemoteTarget $RemoteHost "$RemotePath/deploy/config/"))
Invoke-Checked "scp" @((Join-Path $root "deploy/config/active/service.toml"), (Format-RemoteTarget $RemoteHost "$RemotePath/deploy/config/active/service.toml.example"))
Invoke-Checked "scp" @((Join-Path $root "deploy/config/active/secrets.toml.example"), (Format-RemoteTarget $RemoteHost "$RemotePath/deploy/config/active/"))
Invoke-Checked "scp" @($imageTarPath, (Format-RemoteTarget $RemoteHost "$RemotePath/$remoteImageName"))

$composeFiles = "-f compose.yml"
if ($UseCaddy) {
    $composeFiles = "$composeFiles -f compose.caddy.yml --profile caddy"
}

$dockerCommand = if ($UseSudoDocker) {
    "sudo -n docker"
} else {
    "docker"
}

$probeBaseUrl = if ($PublicBaseUrl) {
    if ($PublicBaseUrl -notmatch "^https?://[^`"\s]+$") {
        throw "PublicBaseUrl must be an HTTP or HTTPS URL without spaces."
    }
    $PublicBaseUrl.TrimEnd("/")
} elseif ($UseCaddy) {
    "__CADDY_FROM_ENV__"
} else {
    "http://127.0.0.1:4310"
}

$remoteTemplate = @'
set -euo pipefail
cd __REMOTE_PATH__

if [ ! -f deploy/.env ]; then
  echo "deploy/.env is missing. Copy deploy/mpgs-server.env.example to deploy/.env and set real values on the server." >&2
  exit 2
fi

if [ ! -f deploy/config/active/service.toml ]; then
  cp -n deploy/config/active/service.toml.example deploy/config/active/service.toml
fi

if [ ! -f deploy/config/active/secrets.toml ] && [ ! -f deploy/config/setup.toml ]; then
  echo "No active secrets.toml or setup.toml found. Configure first-run setup or active service secrets on the server." >&2
  exit 2
fi

__DOCKER_COMMAND__ load -i "__REMOTE_IMAGE_NAME__"
cd deploy
__DOCKER_COMMAND__ compose --project-name "__PROJECT_NAME__" --env-file .env __COMPOSE_FILES__ up -d

probe_base_url="__PROBE_BASE_URL__"
if [ "$probe_base_url" = "__CADDY_FROM_ENV__" ]; then
  caddy_domain="$(grep -E '^CADDY_DOMAIN=' .env | tail -n 1 | cut -d= -f2-)"
  if [ -z "$caddy_domain" ]; then
    echo "CADDY_DOMAIN is missing in deploy/.env, and no PublicBaseUrl was provided." >&2
    exit 2
  fi
  probe_base_url="https://${caddy_domain}"
fi

for attempt in 1 2 3 4 5 6 7 8 9 10; do
  if curl --fail --silent --show-error "$probe_base_url/healthz" >/dev/null; then
    break
  fi
  if [ "$attempt" = "10" ]; then
    echo "healthz probe failed after startup." >&2
    __DOCKER_COMMAND__ compose --project-name "__PROJECT_NAME__" --env-file .env __COMPOSE_FILES__ ps >&2
    exit 1
  fi
  sleep 3
done

curl --fail --silent --show-error "$probe_base_url/api/v1/service-info" >/dev/null
__DOCKER_COMMAND__ compose --project-name "__PROJECT_NAME__" --env-file .env __COMPOSE_FILES__ ps
'@

$remoteScript = $remoteTemplate `
    -replace "__REMOTE_PATH__", ($RemotePath -replace '"', '\"') `
    -replace "__REMOTE_IMAGE_NAME__", ($remoteImageName -replace '"', '\"') `
    -replace "__PROJECT_NAME__", ($ProjectName -replace '"', '\"') `
    -replace "__COMPOSE_FILES__", $composeFiles `
    -replace "__DOCKER_COMMAND__", ($dockerCommand -replace '"', '\"') `
    -replace "__PROBE_BASE_URL__", ($probeBaseUrl -replace '"', '\"')

Invoke-Checked "ssh" @($RemoteHost, $remoteScript)
