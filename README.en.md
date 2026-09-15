[简体中文](README.md) · **[English](README.en.md)**

<p align="center">
  <img src="web/public/app-icon-192.png" alt="LobbyTally icon" width="88" height="88">
</p>

<h1 align="center">LobbyTally</h1>

<p align="center">Find the next game to play with friends.</p>

<p align="center">
  <a href="https://mpgs.lunafleur.dpdns.org/">Try it online</a> ·
  <a href="#preview">Preview</a> ·
  <a href="#install-with-docker">Docker installation (recommended)</a> ·
  <a href="#development-from-source">Development</a> ·
  <a href="https://github.com/Lotulune/lobbytally/issues">Report an issue</a>
</p>

LobbyTally helps friends discover and choose multiplayer games on Steam. Browse new releases and familiar classics, compare player counts, co-op options, private lobbies and dedicated-server information, then use “Want to play” votes to narrow down your next game night.

Use it directly in your browser, or explore the Tauri 2 desktop client included in this repository.

## Preview

![LobbyTally in Chinese: recent releases with recommendation reasons, player counts, reviews and online player estimates](docs/images/lobbytally-feed-zh.png)

*An example of the Chinese interface using the minimal light theme. This screenshot comes from actual use; game data and some interface details may differ from the current version. Both README versions share this Chinese screenshot.*

Game cards bring together recommendation order, multiplayer modes, supported group sizes, reviews, online player estimates and data confidence. Reasons and unverified details are shown together so you can judge whether a game fits your group.

## Features

| Feature | What it helps you do |
| --- | --- |
| Four discovery feeds | Explore recent releases, upcoming games / demos, popular older games and established classics. |
| Preferences and feedback | Set your usual group size and co-op / competitive preferences; record likes, dislikes and games played or owned. |
| Natural-language recommendations | Describe what you want. When configured, AI can analyze candidates and explain suggestions; basic recommendations remain available when AI is unavailable. |
| Community interest | Use “Want to play” votes as another signal when choosing a game. |
| Search and release calendar | Find games, explore release dates and open details for multiplayer information and supporting evidence. |
| Themes and caching | Choose from five themes and adjust visual effects. Previously cached content remains available during network failures, and pending feedback stays queued. |

## Choose a game

1. Open the [web app](https://mpgs.lunafleur.dpdns.org/) and pick a discovery feed.
2. Adjust your preferences in Settings, or describe your group in the recommendation screen—for example: **“Four friends playing this weekend, mostly co-op, preferably with private lobbies.”**
3. Compare reasons, group sizes and multiplayer modes, vote for games you want to play, then check the details on Steam.

Browsing recommendations does not require linking a Steam account. LobbyTally accounts are separate from Steam accounts; sign up or log in for account-related features.

### Reading the recommendations

- The recommendation index is a ranking aid, not a probability that your group will enjoy a game or an official Steam score.
- A store's multiplayer tag does not confirm private lobbies, co-op or dedicated servers. Missing information remains marked as unknown or unverified.
- Prices, reviews, online player counts and release dates change. Confirm purchase details on Steam.
- Steam friend syncing, shared-library matching and automatic matchmaking are not currently provided. See [Known limitations](docs/KNOWN_LIMITATIONS.md) for more detail.

## Install with Docker

**Docker is the recommended way to self-host LobbyTally.** Use prebuilt images without installing Rust or Node.js, or compiling on your server. The default configuration runs the Web UI, API and continuous ingestion worker.

These commands target a **Linux x86_64 / amd64** host. Prepare Docker Engine, the Docker Compose plugin (`docker compose`), Git, curl, OpenSSL, and system tools providing `flock` and `timeout`. Your current user must be able to run Docker.

### 1. Prepare the installation directory

For a first installation in a new directory:

```bash
git clone https://github.com/Lotulune/lobbytally.git
cd lobbytally
cp deploy/.env.example deploy/.env
cp deploy/mpgs.env.example deploy/mpgs.env
mkdir -p deploy/runtime
chmod 600 deploy/.env deploy/mpgs.env
```

### 2. Configure your instance

Keep `MPGS_DEPLOY_MODE=full` in `deploy/.env` to run the Web UI, API and worker together. Edit `deploy/mpgs.env` and configure at least the following:

| Setting | What to use |
| --- | --- |
| `MPGS_ADMIN_TOKEN` | A strong random token, generated with the command below. Do not leave it empty or use an example token. |
| `MPGS_CORS_ALLOWED_ORIGINS` | For local access, use `http://localhost:18082,http://127.0.0.1:18082,http://tauri.localhost,tauri://localhost`. When using your own domain, add its actual HTTPS origin and replace the hosted demo's domain from the template. |
| `MPGS_TRUST_PROXY_HEADERS` | Set to `false` for direct local access. Enable it according to the operations guide after configuring a trusted reverse proxy. |
| `MPGS_AI_PROVIDER` | Add `MPGS_AI_PROVIDER=disabled` if you are not configuring AI yet. Basic recommendations remain available. |

```bash
openssl rand -hex 32
```

`MPGS_STEAM_WEB_API_KEY` is optional and enables official Steam catalog synchronization. AI endpoints, credentials and models are also optional. Keep credentials in the instance's environment files.

### 3. Install a published release

Read the validated image revision from the release pointer, then let the repository's updater pull matching server and Web images and run health checks:

```bash
set -eu
docker pull ghcr.io/lotulune/mpgs-server:release-main
release_sha="$(docker image inspect \
  --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' \
  ghcr.io/lotulune/mpgs-server:release-main)"
test -n "$release_sha"
MPGS_RELEASE_SHA="$release_sha" ./deploy/update.sh
```

This revision applies only to the installation command and is not saved as a permanent pin. It lets you install a published release even when documentation on `main` is newer than the latest image. If GHCR requires authentication, run `docker login ghcr.io` and retry the pull.

### 4. Open and check the app

```bash
curl --fail http://127.0.0.1:18082/health/ready
docker compose --env-file deploy/.env -f deploy/docker-compose.yml ps
```

Open [http://localhost:18082](http://localhost:18082) on the installation host. On a VPS, configure your HTTPS domain to reverse-proxy to `127.0.0.1:18082`. Ports bind to loopback by default, so the app is not directly accessible through the VPS's public IP. The standalone API listens on `127.0.0.1:18081`.

The database, avatars and backups persist in `deploy/runtime/`; retain this directory when updating containers. A new instance starts with an empty catalog that the worker gradually populates. It does not copy the hosted site's catalog automatically.

### Updates and maintenance

Run from the existing installation directory:

```bash
./deploy/update.sh
```

The updater checks the validated release pointer for updates; documentation-only commits may have no new images. On systemd hosts, you can also configure automatic checks every five minutes. See the [Operations guide](docs/OPERATIONS.md) for HTTPS, automatic updates, backups and recovery, and backend-only mode.

## Development from source

Use this path when changing the code or developing the desktop client. For everyday self-hosting, prefer the Docker installation above.

Requirements: **Rust 1.97+, Node.js 22, pnpm 9.15.9 and Git**. The PowerShell example below starts a local environment with demo data and requires no Steam or AI API keys.

### 1. Get the code

```powershell
git clone https://github.com/Lotulune/lobbytally.git
cd lobbytally
pnpm install --frozen-lockfile
```

### 2. Start the API

Run from the repository root:

```powershell
New-Item -ItemType Directory -Force data | Out-Null
$env:MPGS_DATABASE_PATH = '.\data\demo.db'
$env:MPGS_SEED_DEMO = 'true'
$env:MPGS_ADMIN_TOKEN = 'local-demo-only-token'
$env:MPGS_AI_PROVIDER = 'disabled'
cargo run -p mpgs-server --locked
```

The API listens on `http://127.0.0.1:17880` by default. This token and demo dataset are for local development only. For real catalog ingestion, AI configuration and production credentials, see the [Development guide](docs/DEVELOPMENT.md) and [Operations guide](docs/OPERATIONS.md).

### 3. Start the Web app

Leave the API running and open a second terminal at the repository root:

```powershell
pnpm web:dev
```

Open [http://localhost:5173](http://localhost:5173). Vite proxies API requests to the local server.

### Check your changes

```powershell
pnpm web:test
pnpm web:build
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Desktop development requires additional platform dependencies; see the [Desktop / Web client guide](web/README.md).

## Technology and layout

The UI uses **React, TypeScript and Vite**, with **Tauri 2** as the desktop shell. The backend uses **Rust, Axum and Tokio**, with **SQLite** for the catalog, recommendation data and feedback. Deterministic rules power the base recommendations; AI is an optional enhancement.

| Path | Responsibility |
| --- | --- |
| `web/` | Shared browser / desktop UI, themes, caching and feedback interactions. |
| `apps/server/` | HTTP API, sessions and accounts, recommendations and job scheduling. |
| `apps/desktop/` | Tauri desktop client. |
| `apps/dbtool/` | Ingestion, auditing, migrations and backup tooling. |
| `crates/` | Domain types, recommender, Steam data adapters, storage and AI. |
| `migrations/` | SQLite schema migrations. |
| `deploy/` | Container configuration, worker and deployment scripts. |
| `docs/` | Product, architecture, development and operations documentation. |

Clients access the server through its API. The authoritative SQLite database and the processes accessing it run on the same host; database files are not shared over a network filesystem.

## Documentation

Most detailed documentation is currently written in Chinese.

| Document | Contents |
| --- | --- |
| [Development guide](docs/DEVELOPMENT.md) | Setup, local development, ingestion and validation commands. |
| [Operations guide](docs/OPERATIONS.md) | Self-hosting, configuration, backups, upgrades and rollback. |
| [HTTP API](docs/API.md) | Endpoints and data contracts. |
| [Architecture](docs/ARCHITECTURE.md) | Component responsibilities and client / server boundaries. |
| [Recommendation algorithm](docs/RECOMMENDATION.md) | Ranking signals, explanations and calibration. |
| [AI integration](docs/AI.md) | Providers, retrieval, configuration and fallback behavior. |
| [Known limitations](docs/KNOWN_LIMITATIONS.md) | Current data, product and technical constraints. |
| [Privacy](docs/PRIVACY.md) | Data handling and privacy practices. |

Use [Issues](https://github.com/Lotulune/lobbytally/issues) to report problems, incorrect game data or feature ideas. Include reproduction steps, your platform and relevant screenshots. Do not include API keys, access tokens or personal databases.
