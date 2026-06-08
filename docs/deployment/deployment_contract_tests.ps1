$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")

$compose = Get-Content -Raw -Path (Join-Path $root "deploy\compose.yml")
$caddyCompose = Get-Content -Raw -Path (Join-Path $root "deploy\compose.caddy.yml")
$caddyfile = Get-Content -Raw -Path (Join-Path $root "deploy\Caddyfile")
$dockerfile = Get-Content -Raw -Path (Join-Path $root "Dockerfile.mpgs-server")
$deploymentDoc = Get-Content -Raw -Path (Join-Path $root "docs\deployment\mpgs-server-compose.md")
$serviceConfigExample = Get-Content -Raw -Path (Join-Path $root "deploy\config\active\service.toml")
$secretsExample = Get-Content -Raw -Path (Join-Path $root "deploy\config\active\secrets.toml.example")
$setupExample = Get-Content -Raw -Path (Join-Path $root "deploy\config\setup.toml.example")
$localBuildScript = Get-Content -Raw -Path (Join-Path $root "deploy\scripts\build-mpgs-server-image.ps1")
$remoteDeployScript = Get-Content -Raw -Path (Join-Path $root "deploy\scripts\deploy-mpgs-server-remote.ps1")

if ($compose -notmatch 'postgres:16-bookworm') {
    throw "compose.yml must define Postgres 16."
}
if ($compose -notmatch '127\.0\.0\.1:4310:4310') {
    throw "compose.yml must bind mpgs-server to localhost by default."
}
if ($compose -notmatch '/healthz') {
    throw "compose.yml must healthcheck /healthz."
}
if ($compose -notmatch 'MPGS_CONFIG_DIR') {
    throw "compose.yml must locate the server config directory with MPGS_CONFIG_DIR."
}
if ($compose -notmatch './config:/var/lib/mpgs/config' -or $compose -match './config:/var/lib/mpgs/config:ro') {
    throw "compose.yml must mount the server config directory writable for setup-managed TOML files."
}
if ($compose -match 'MPGS_DATABASE_URL') {
    throw "compose.yml must not put service settings or database URL in environment variables."
}
if ($caddyCompose -notmatch '--profile caddy' -and $deploymentDoc -notmatch '--profile caddy') {
    throw "deployment docs must describe the caddy profile."
}
if ($caddyfile -notmatch 'reverse_proxy mpgs-server:4310') {
    throw "Caddyfile must reverse proxy to mpgs-server:4310."
}
if ($dockerfile -notmatch 'cargo build --release -p mpgs-server') {
    throw "Dockerfile must build only the mpgs-server package."
}
if ($deploymentDoc -notmatch 'Do not build on the VPS' -or $deploymentDoc -notmatch 'must not compile Rust') {
    throw "deployment docs must forbid VPS builds."
}
if ($deploymentDoc -notmatch 'active/service.toml' -or $deploymentDoc -notmatch 'active/secrets.toml') {
    throw "deployment docs must describe the active TOML config files."
}
if ($serviceConfigExample -notmatch '\[service_connection\]' -or $serviceConfigExample -notmatch 'public_base_url') {
    throw "active service config example must include the public service connection URL."
}
if ($serviceConfigExample -notmatch '\[public_cors\]' -or $serviceConfigExample -notmatch 'allow_any_origin') {
    throw "active service config example must include the public read CORS setting."
}
if ($serviceConfigExample -notmatch '\[deployment\]' -or $serviceConfigExample -notmatch 'restart_policy = "compose:unless-stopped"') {
    throw "active service config example must include the Compose restart policy metadata."
}
if ($deploymentDoc -notmatch 'service_connection\.public_base_url' -or $deploymentDoc -notmatch 'connection-share API') {
    throw "deployment docs must describe the public base URL used for keyless connection sharing."
}
if ($deploymentDoc -notmatch 'public_cors' -or $deploymentDoc -notmatch 'Management, setup, and restart routes stay same-origin') {
    throw "deployment docs must describe public-only CORS boundaries."
}
if ($deploymentDoc -notmatch 'only locates the config directory') {
    throw "deployment docs must state that .env only locates config, not service secrets."
}
if ($secretsExample -notmatch '\[admin\]' -or $secretsExample -notmatch 'token_hash' -or $secretsExample -notmatch 'session_secret') {
    throw "secrets.toml.example must include admin token hash and session secret placeholders."
}
if ($setupExample -notmatch '\[setup\]' -or $setupExample -notmatch 'token_hash') {
    throw "setup.toml.example must include a setup token hash placeholder."
}
if ($deploymentDoc -notmatch 'setup.toml' -or $deploymentDoc -notmatch 'setup token') {
    throw "deployment docs must describe setup token configuration."
}
if ($deploymentDoc -notmatch 'deploy/config/pending' -or $deploymentDoc -notmatch 'restartRequired=true') {
    throw "deployment docs must describe pending config and restart-required state."
}
if ($deploymentDoc -notmatch 'validates pending service configuration before promoting it to active') {
    throw "deployment docs must describe startup pending config validation and promotion."
}
if ($deploymentDoc -notmatch '/api/v1/admin/restart' -or $deploymentDoc -notmatch 'restart: unless-stopped') {
    throw "deployment docs must describe the managed restart API and Compose restart policy."
}
if ($deploymentDoc -notmatch '/api/v1/admin/diagnostics' -or $deploymentDoc -notmatch 'public base URL' -or $deploymentDoc -notmatch 'provider configuration presence') {
    throw "deployment docs must describe admin diagnostics for deployment status."
}
if ($deploymentDoc -notmatch 'does not use the Docker socket' -or $deploymentDoc -notmatch 'restart-helper' -or $deploymentDoc -notmatch 'host commands') {
    throw "deployment docs must forbid Docker socket, restart-helper, and host command restart control."
}
if ($deploymentDoc -notmatch 'must not clear Steam, LLM, R2, or admin credentials') {
    throw "deployment docs must document pending secret inheritance boundaries."
}
if ($deploymentDoc -notmatch 'Do not put the raw admin token') {
    throw "deployment docs must forbid storing the raw admin token."
}
if ($deploymentDoc -notmatch 'Do not put the raw setup token') {
    throw "deployment docs must forbid storing the raw setup token."
}
if ($localBuildScript -notmatch 'Dockerfile\.mpgs-server' -or $localBuildScript -notmatch 'docker save' -or $localBuildScript -notmatch 'buildx') {
    throw "local build script must build and save the server image locally, including buildx platform support."
}
if ($remoteDeployScript -notmatch 'docker load' -or $remoteDeployScript -notmatch 'up -d' -or $remoteDeployScript -notmatch '/healthz' -or $remoteDeployScript -notmatch '/api/v1/service-info') {
    throw "remote deploy script must load the image, start compose, and probe healthz plus service-info."
}
if ($remoteDeployScript -notmatch 'deploy/config/active/service.toml') {
    throw "remote deploy script must upload the active service config example for first manual configuration."
}
if ($remoteDeployScript -match 'cargo build|rustc|docker build|docker compose build|npm run|pnpm|yarn') {
    throw "remote deploy script must not compile or build artifacts on the VPS."
}
if ($deploymentDoc -notmatch 'deploy/scripts/build-mpgs-server-image.ps1' -or $deploymentDoc -notmatch 'deploy/scripts/deploy-mpgs-server-remote.ps1') {
    throw "deployment docs must describe the checked local build and remote deploy scripts."
}
if ($deploymentDoc -notmatch 'linux/arm64' -or $deploymentDoc -notmatch 'ora_vps') {
    throw "deployment docs must describe local arm64 image creation for ora_vps-style servers."
}
if ($deploymentDoc -notmatch 'does not overwrite remote `deploy/.env`, active secrets, or active service config') {
    throw "deployment docs must state that remote deployment does not overwrite server secrets or active config."
}

Write-Output "Deployment contract checks passed."
