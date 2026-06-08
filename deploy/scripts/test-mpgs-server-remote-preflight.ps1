param(
    [Parameter(Mandatory = $true)]
    [string] $RemoteHost,

    [string] $RemotePath = "~/mpgs-server",
    [string] $ImageTar = "",
    [switch] $UseCaddy,
    [switch] $UseSudoDocker,
    [string] $PublicBaseUrl = ""
)

$ErrorActionPreference = "Stop"

function Require-Command {
    param([string] $Name)

    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "$Name is required on the local preflight machine."
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

function Invoke-RemoteBashScript {
    param(
        [string] $RemoteHostName,
        [string] $Script
    )

    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    $encodedScript = [Convert]::ToBase64String(
        $utf8NoBom.GetBytes($Script.TrimStart([char]0xFEFF))
    )
    & ssh $RemoteHostName "printf '%s' '$encodedScript' | base64 -d | bash -s"
    if ($LASTEXITCODE -ne 0) {
        throw "ssh failed with exit code $LASTEXITCODE."
    }
}

$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
if ($ImageTar) {
    $imageTarPath = if ([System.IO.Path]::IsPathRooted($ImageTar)) {
        $ImageTar
    } else {
        Join-Path $root $ImageTar
    }

    if (-not (Test-Path $imageTarPath)) {
        throw "Image archive not found locally: $imageTarPath. Build it locally or in CI first."
    }
}

Require-Command "ssh"

if ($RemotePath -match "\s" -or $RemotePath -match "'") {
    throw "RemotePath must not contain whitespace or single quotes."
}
if ($RemotePath -notmatch "^[A-Za-z0-9_./~:-]+$") {
    throw "RemotePath contains unsupported shell characters."
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

# Keep these literal command forms visible for deployment contract checks:
# docker compose version
# sudo -n docker compose version

$remoteTemplate = @'
set -euo pipefail

echo "host=$(hostname)"
echo "arch=$(uname -m)"
. /etc/os-release
echo "os=${ID}:${VERSION_ID}"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is not installed on the remote host." >&2
  exit 2
fi

__DOCKER_COMMAND__ --version >/dev/null
__DOCKER_COMMAND__ compose version >/dev/null

remote_path="__REMOTE_PATH__"
if [ "${remote_path#~/}" != "$remote_path" ]; then
  remote_path="${HOME}/${remote_path#~/}"
fi
if [ -d "$remote_path" ]; then
  echo "deploy_root=exists"
else
  echo "deploy_root=missing"
fi

if [ -f "$remote_path/deploy/.env" ]; then
  echo "env_file=exists"
  sed -n 's/^\([^#=][^=]*\)=.*/env_key=\1=<redacted>/p' "$remote_path/deploy/.env" | sort
else
  echo "env_file=missing"
fi

if [ -f "$remote_path/deploy/config/active/service.toml" ]; then
  echo "active_service=exists"
  grep -E '^(\[service_connection\]|\[public_cors\]|\[deployment\]|public_base_url|allow_any_origin|restart_policy)' "$remote_path/deploy/config/active/service.toml" || true
else
  echo "active_service=missing"
fi

if [ -f "$remote_path/deploy/config/active/secrets.toml" ]; then
  echo "active_secrets=exists"
else
  echo "active_secrets=missing"
fi

if [ -f "$remote_path/deploy/config/setup.toml" ]; then
  echo "setup_toml=exists"
else
  echo "setup_toml=missing"
fi

public_port_pattern='(^| )caddy|(^|[^0-9]):(80|443)->'
if ! docker_ps_output="$(__DOCKER_COMMAND__ ps --format '{{.Names}} {{.Ports}}' 2>&1)"; then
  echo "$docker_ps_output" >&2
  exit 2
fi

if printf '%s\n' "$docker_ps_output" | grep -E "$public_port_pattern" >/dev/null 2>&1; then
  echo "public_ports=occupied"
  printf '%s\n' "$docker_ps_output" | grep -E "$public_port_pattern"
else
  echo "public_ports=available"
fi

probe_base_url="__PROBE_BASE_URL__"
if [ "$probe_base_url" = "__CADDY_FROM_ENV__" ]; then
  if [ -f "$remote_path/deploy/.env" ]; then
    caddy_domain="$(grep -E '^CADDY_DOMAIN=' "$remote_path/deploy/.env" | tail -n 1 | cut -d= -f2-)"
  else
    caddy_domain=""
  fi
  if [ -n "$caddy_domain" ]; then
    probe_base_url="https://${caddy_domain}"
  else
    echo "CADDY_DOMAIN=missing"
    probe_base_url="https://CADDY_DOMAIN"
  fi
fi

echo "health_probe=${probe_base_url}/healthz"
echo "service_info_probe=${probe_base_url}/api/v1/service-info"
echo "admin_probe=${probe_base_url}/admin"
'@

$remoteScript = $remoteTemplate `
    -replace "__REMOTE_PATH__", ($RemotePath -replace '"', '\"') `
    -replace "__DOCKER_COMMAND__", ($dockerCommand -replace '"', '\"') `
    -replace "__PROBE_BASE_URL__", ($probeBaseUrl -replace '"', '\"')

Invoke-RemoteBashScript $RemoteHost $remoteScript
