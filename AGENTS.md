# AGENTS.md

## Project and deployment

- LobbyTally uses a Rust/Axum API and React/Vite frontend in one Vercel Container Runtime service. Supabase provides PostgreSQL, private avatar storage, Vault, and Cron. Production does not use the historical VPS, GHCR, SQLite, or persistent worker deployment path.
- Before deployment work, read [docs/AGENT_DEPLOYMENT.md](docs/AGENT_DEPLOYMENT.md). Provider setup is in [docs/VERCEL_SUPABASE.en.md](docs/VERCEL_SUPABASE.en.md), with a [Chinese guide](docs/VERCEL_SUPABASE.md).
- Use the latest cloud section of [docs/PRODUCTION_STATUS.md](docs/PRODUCTION_STATUS.md) to identify an existing deployment only when the user intends to update that deployment. Never use its project IDs, organization, domain, or DNS records as defaults for a fresh clone.
- These instructions do not authorize deployments, migrations, DNS changes, commits, or pushes. Honor the current user's scope and existing authorization; finish authorized preparation before requesting a missing approval.

## Implementation boundaries

- Preserve the existing `/v1` API and application account/session model. Supabase Auth is not part of this deployment.
- Keep worker work bounded and resumable. Database leases, cursors, and retries must survive replacement of a Vercel instance.
- Keep `mpgs` private, avatars private, and secrets server-side. Use the restricted database runtime role and the session pooler on port `5432`; preserve certificate and hostname verification.
- Review `.vercelignore` and `.dockerignore` when adding build inputs. Never upload local databases, credentials, CLI state, caches, or build artifacts.

## Verification

- Match verification to the change. For documentation or agent instructions, review the diff, paths, links, and command accuracy; do not run the application suites solely for prose changes.
- For behavior changes, finish implementation, run the affected checks, and fix failures before handoff. PostgreSQL integration tests require an expendable `MPGS_TEST_DATABASE_URL`; never point them at production.
- CI change routing chooses whether heavy jobs run. Skipped jobs on a documentation-only change are expected; do not force a full run without a concrete need.
