[简体中文](README.md) · **[English](README.en.md)**

<p align="center">
  <img src="web/public/app-icon-192.png" alt="LobbyTally icon" width="88" height="88">
</p>

<h1 align="center">LobbyTally</h1>

<p align="center">Find the next game to play with friends.</p>

<p align="center">
  <a href="https://mpgs.lunafleur.dpdns.org/">Try it online</a> ·
  <a href="#preview">Preview</a> ·
  <a href="docs/VERCEL_SUPABASE.en.md">Cloud deployment</a> ·
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

## Deploy with Vercel and Supabase

The current deployment serves the **React / Vite frontend and Rust / Axum API from one Vercel container service**. [Dockerfile.vercel](Dockerfile.vercel) builds both, and [vercel.json](vercel.json) selects Tokyo (`hnd1`). Supabase provides PostgreSQL, private avatar storage, and Cron. Durable server-side state lives in the cloud, with no VPS or persistent worker.

| Component | Current implementation |
| --- | --- |
| Database | A private `mpgs` schema in Supabase PostgreSQL, accessed through the Session pooler on port `5432`. |
| Accounts | The application's own Rust account and session implementation; Supabase Auth setup is not required. |
| Avatars | A private Supabase Storage bucket named `avatars`, with avatar access served by the API. |
| Ingestion and maintenance | Supabase Cron triggers a bounded maintenance batch every minute and avatar cleanup every five minutes. PostgreSQL stores cursors and leases. |
| AI | Optional; `MPGS_AI_PROVIDER=disabled` by default. Base recommendations remain available. |

For deployment, prepare Git, Node.js 22, Supabase and Vercel accounts, and a local clone of this repository. Run the commands below from the repository root; Vercel builds Rust and the frontend in the container.

### 1. Initialize a new Supabase project

Create a Supabase project in a region close to Vercel's `hnd1`. From the repository root, use the verified CLI version to link the project and apply migrations:

```powershell
npx --yes supabase@2.121.0 login
npx --yes supabase@2.121.0 init
npx --yes supabase@2.121.0 link --project-ref YOUR_PROJECT_REF
npx --yes supabase@2.121.0 db push --linked --skip-vault --dry-run
npx --yes supabase@2.121.0 db push --linked --skip-vault
npx --yes supabase@2.121.0 migration list --linked
```

Run `init` for the initial setup when `supabase/config.toml` does not yet exist locally. Migrations create the empty catalog, runtime defaults, avatar bucket, scheduling function, and the `lobbytally_runtime` backend role. Its generated password is stored in Vault as `lobbytally_database_password`. Keep `mpgs` private; browsers access data through the `/v1` API.

This flow is for a new project. If you already initialized the database through the SQL Editor, first follow the [deployment guide](docs/VERCEL_SUPABASE.en.md) to verify and record migration history before using `db push`. Existing SQLite accounts and game data are not imported.

### 2. Configure Vercel and server environment variables

Import the repository into Vercel with **Root Directory set to the repository root, `.`**. Use the repository's Container Runtime configuration and `Dockerfile.vercel`. If you choose another Supabase region, update the single-region setting in `vercel.json` accordingly.

Set the following in the Vercel project's **Production Environment Variables**. See [deploy/cloud.env.example](deploy/cloud.env.example) for the complete template:

| Variable | Value or source |
| --- | --- |
| `PORT` | `8080`. |
| `MPGS_DATABASE_URL` | `postgresql://lobbytally_runtime.PROJECT_REF:PASSWORD@POOLER_HOST:5432/postgres?sslmode=require`; copy the host from Supabase **Connect → Session pooler**, use the Vault password, and percent-encode special URL characters. |
| `MPGS_SUPABASE_URL` | `https://PROJECT_REF.supabase.co`. |
| `MPGS_SUPABASE_SERVICE_ROLE_KEY` | The Supabase server-side service role key. |
| `MPGS_CRON_SECRET` | An independently generated random value of at least 32 non-whitespace bytes; use the same value in Vault below. |
| `MPGS_TRUST_PROXY_HEADERS` / `MPGS_RATE_LIMIT_ENABLED` | Set both to `true`. |
| `MPGS_AI_PROVIDER` | `disabled`; add the provider settings from the template when enabling AI. |
| `MPGS_ADMIN_TOKEN` | Optional; configure a separate token to use the admin API. |
| `MPGS_STEAM_WEB_API_KEY` | Optional; enables Steam's official catalog sync. Store candidate discovery works without it. |

Keep credentials server-side, without a `VITE_` prefix or a Git commit. The current driver requires the **Session pooler on port 5432**; the Transaction pooler on port 6543 is unsupported. Use a separate Supabase project and credentials for Preview deployments.

Deploy through Vercel's Git integration, or link the same project and publish from the repository root with the verified CLI version:

```powershell
npx --yes vercel@64.5.0 login
npx --yes vercel@64.5.0 link --scope YOUR_VERCEL_TEAM
npx --yes vercel@64.5.0 deploy --dry --json --scope YOUR_VERCEL_TEAM
npx --yes vercel@64.5.0 deploy --prod --skip-domain --scope YOUR_VERCEL_TEAM
npx --yes vercel@64.5.0 inspect YOUR_DEPLOYMENT_ID --json --scope YOUR_VERCEL_TEAM
```

After the deployment reaches `readyState=READY` and `/health/ready` returns application JSON, run `npx --yes vercel@64.5.0 promote YOUR_DEPLOYMENT_ID --scope YOUR_VERCEL_TEAM`. The [deployment guide](docs/VERCEL_SUPABASE.en.md) explains protected-deployment checks. Select Standard Protection in Deployment Protection, then check the public production domain again:

```powershell
$siteOrigin = 'https://YOUR_PROJECT.vercel.app'
Invoke-RestMethod "$siteOrigin/health/live"
Invoke-RestMethod "$siteOrigin/health/ready"
Invoke-RestMethod "$siteOrigin/v1/meta"
```

`/health/ready` should return `status: ready` and `database: ok`. The production domain must allow public access so Supabase Cron can reach it; a Vercel sign-in page does not pass this check. An initialized empty catalog is healthy.

### 3. Enable ingestion and verify scheduled work

In Supabase Vault, set `lobbytally_site_origin` to the production HTTPS origin without an API path. Set `lobbytally_cron_secret` to the exact value of Vercel's `MPGS_CRON_SECRET`, then run:

```powershell
npx --yes supabase@2.121.0 db query --linked --file supabase/enable-schedules.sql
```

The jobs call `POST /internal/v1/maintenance` and `POST /internal/v1/avatar-cleanup`. Batches gradually populate the game catalog. Verify the Cron history, actual HTTP responses, and advancing database cursors together. To pause scheduled work, run [supabase/disable-schedules.sql](supabase/disable-schedules.sql).

**Keep using the same Supabase and Vercel projects for updates.** When new migrations exist, inspect migration history and apply pending files with `db push --linked --skip-vault` before deploying. Application-only updates require a redeployment, without recreating the database. See the [English deployment guide](docs/VERCEL_SUPABASE.en.md) / [中文部署指南](docs/VERCEL_SUPABASE.md) for acceptance checks, domain changes, and secret rotation. Agents should first read [AGENTS.md](AGENTS.md) and the [agent deployment guide](docs/AGENT_DEPLOYMENT.md).

## Development from source

Use this path when changing the code or developing the desktop client. Deploy production with the Vercel + Supabase workflow above.

Requirements: **Rust 1.97+, Node.js 22, pnpm 9.15.9, Git, and PostgreSQL 17+**. Configure the database and server environment as described in the [deployment guide](docs/VERCEL_SUPABASE.en.md).

### 1. Get the code

```powershell
git clone https://github.com/Lotulune/lobbytally.git
cd lobbytally
pnpm install --frozen-lockfile
```

### 2. Start the API

Export `MPGS_DATABASE_URL`, Supabase Storage, and Cron credentials into the current terminal. Initialize a new local PostgreSQL database with a connection that has migration privileges:

```powershell
cargo run -p mpgs-dbtool --locked -- migrate
```

For an initialized database, start the API directly:

```powershell
cargo run -p mpgs-server --locked
```

The API listens on `http://127.0.0.1:17880` by default. Local `dbtool migrate` initializes the application schema; configure Supabase Storage, Vault, and Cron through the deployment steps above. An initialized empty catalog can start successfully; Supabase Cron triggers production ingestion.

### 3. Start the Web app

Leave the API running and open a second terminal at the repository root:

```powershell
pnpm web:dev
```

Open [http://localhost:5173](http://localhost:5173). Vite proxies API requests to the local server.

### Check your changes

Choose checks that match the change. For documentation-only changes, review the content, diff, and links. Before running database tests, set `MPGS_TEST_DATABASE_URL` to a separate disposable PostgreSQL test database.

```powershell
pnpm web:test
pnpm web:build
cargo fmt --all -- --check
cargo test --workspace --locked -- --test-threads=4
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Desktop development requires additional platform dependencies; see the [Desktop / Web client guide](web/README.md).

### How CI runs

The [CI workflow](.github/workflows/ci.yml) uses `CI routing` to classify changes and `CI result` to summarize the outcome:

- Documentation-only changes take the lightweight path, skipping Web, Rust, Linux release, and container builds.
- Code changes run Web tests and builds, Rust formatting / tests / Clippy, a Linux build, and the Vercel container build.
- A manual run with `full=false` performs the normal checks. `full=true` or a `v*` tag adds the full desktop and multiplatform checks.

Pull requests use the complete diff against their target branch; pushes use the pushed commit range. A PR containing code still runs normal checks when its latest commit only edits a README. Unknown change ranges also run normal checks.

When configuring branch protection, use the stable `CI result` as the required check. View remote GitHub CI results in the commit's Actions run; local validation and cloud deployment are separate operations.

This routing controls GitHub Actions only. Separately connected Vercel Git deployments follow the Vercel project's build-trigger settings.

## Technology and layout

The UI uses **React, TypeScript and Vite**, with **Tauri 2** as the desktop shell. The backend uses **Rust, Axum and Tokio**, with **Supabase PostgreSQL** for the catalog, recommendation data and feedback. Deterministic rules power the base recommendations; AI is an optional enhancement.

| Path | Responsibility |
| --- | --- |
| `web/` | Shared browser / desktop UI, themes, caching and feedback interactions. |
| `apps/server/` | HTTP API, sessions and accounts, recommendations, static frontend, and authenticated maintenance endpoints. |
| `apps/desktop/` | Tauri desktop client. |
| `apps/dbtool/` | PostgreSQL migrations, ingestion and audit CLI, and isolated local E2E fixtures. |
| `crates/worker/` | Ingestion and maintenance batches with time budgets, durable cursors, and leases. |
| `crates/` | Domain types, recommender, Steam data adapters, PostgreSQL storage, and AI. |
| `supabase/` | PostgreSQL migrations, private storage, and Cron configuration. |
| `Dockerfile.vercel` / `vercel.json` | Frontend and API container build, region, routing, and execution limits. |
| `deploy/` | Current cloud environment template and retained historical self-hosting scripts. |
| `docs/` | Product, architecture, development and operations documentation. |

Clients access the backend through its API. Supabase owns durable state; Vercel instances do not rely on local database files or permanent background processes.

## Documentation

The current deployment guide is available in English and Chinese. Most product and design documents are written in Chinese.

| Document | Contents |
| --- | --- |
| [Vercel + Supabase deployment](docs/VERCEL_SUPABASE.en.md) | Current setup, migrations, updates, secret handling, and cloud acceptance checks. |
| [Agent deployment guide](docs/AGENT_DEPLOYMENT.md) / [AGENTS.md](AGENTS.md) | Entry points, constraints, and verification steps for agents working on the project. |
| [HTTP API](docs/API.md) | Endpoints and data contracts. |
| [Architecture](docs/ARCHITECTURE.md) | Component responsibilities and client / server boundaries. |
| [Recommendation algorithm](docs/RECOMMENDATION.md) | Ranking signals, explanations and calibration. |
| [AI integration](docs/AI.md) | Providers, retrieval, configuration and fallback behavior. |
| [Known limitations](docs/KNOWN_LIMITATIONS.md) | Current data, product and technical constraints. |
| [Privacy](docs/PRIVACY.md) | Data handling and privacy practices. |
| [Historical development guide](docs/DEVELOPMENT.md) / [historical operations guide](docs/OPERATIONS.md) | Retained SQLite / VPS material. Use this README and the cloud deployment guide for current deployment instructions. |

Use [Issues](https://github.com/Lotulune/lobbytally/issues) to report problems, incorrect game data or feature ideas. Include reproduction steps, your platform and relevant screenshots. Do not include API keys, access tokens or personal databases.
