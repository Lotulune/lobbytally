# Deploy with Vercel and Supabase

[简体中文](VERCEL_SUPABASE.md) · **English** · [Agent deployment guide](AGENT_DEPLOYMENT.md)

The React/Vite frontend and Rust/Axum API are built together by `Dockerfile.vercel` and deployed as one Vercel Container Runtime service. Supabase provides PostgreSQL, private avatar storage, Vault, and Cron. Browsers use the Rust API, including the application's own account authentication.

A first deployment starts with an empty database; scheduled batches populate the catalog. Updates preserve existing data and job progress. Use your own projects for the placeholders below. [PRODUCTION_STATUS.md](PRODUCTION_STATUS.md) records the maintained production installation.

The commands use versions verified during this deployment: Supabase CLI `2.121.0` and Vercel CLI `64.5.0`. Check the relevant `--help` before upgrading them. The repository defaults to Vercel Tokyo `hnd1`, paired with Supabase Tokyo `ap-northeast-1`; change the region configuration if you deploy elsewhere.

## 1. Initialize a new Supabase project

Create the project in the intended organization through the [Supabase dashboard](https://supabase.com/dashboard), then run from the repository root:

```text
npx --yes supabase@2.121.0 login
npx --yes supabase@2.121.0 init
npx --yes supabase@2.121.0 link --project-ref YOUR_PROJECT_REF
npx --yes supabase@2.121.0 migration list --linked
npx --yes supabase@2.121.0 db push --linked --skip-vault --dry-run
npx --yes supabase@2.121.0 db push --linked --skip-vault
```

Reuse existing authentication and links after checking the target. Skip `init` if `supabase/config.toml` exists. `--skip-vault` disables the CLI's synchronization from `config.toml`; Vault statements inside migration SQL still run.

The initialization and subsequent migrations currently contain these files in filename order:

| Migration | Purpose |
| --- | --- |
| `20261018174837_lobbytally_initial.sql` | Private `mpgs` schema, business tables, indexes, and runtime defaults |
| `20261018174851_lobbytally_platform.sql` | Private `avatars` bucket, Cron / HTTP extensions, and dispatch function |
| `20261018175423_lobbytally_postgres_hardening.sql` | Extension placement and foreign-key indexes |
| `20261018182444_lobbytally_runtime_access.sql` | Dedicated backend role, RLS policies, and a random password encrypted in Vault |
| `20261018203956_lobbytally_release_transition_index.sql` | Partial index for release-transition freshness queries |

The fifth migration is an additive performance index, compatible with existing data and the running application. It keeps the application schema contract at version `1`; Supabase CLI records the additional migration separately. An installation with the first four migrations already applied needs only the new file.

Alternatively, execute the files sequentially in the SQL Editor. The editor does not record CLI migration history. Only after confirming that all five files succeeded, register those applied versions before using CLI pushes:

```text
npx --yes supabase@2.121.0 migration repair 20261018174837 20261018174851 20261018175423 20261018182444 20261018203956 --status applied
```

Keep `mpgs` outside the Data API's exposed schemas. Browser clients need no Supabase key, and the Rust API controls access to private avatars.

## 2. Configure Vercel and server environment variables

Import the repository into [Vercel](https://vercel.com/new), choosing the **repository root** as Root Directory. `services.app` in `vercel.json` points to `Dockerfile.vercel`. Its `maxDuration: 120` belongs under `services.app.functions`. Remote builds do not require local Docker; confirm Container Runtime availability against the [current documentation](https://vercel.com/docs/functions/runtimes/container) and your account's build result.

Copy [deploy/cloud.env.example](../deploy/cloud.env.example) to the ignored `deploy/cloud.env`, fill it privately, and configure the values in Vercel's **Production** environment. The local file is not automatically uploaded as environment configuration.

```text
npx --yes vercel@64.5.0 login
npx --yes vercel@64.5.0 link --scope YOUR_VERCEL_TEAM
npx --yes vercel@64.5.0 env add MPGS_DATABASE_URL production --sensitive --scope YOUR_VERCEL_TEAM
```

`env add` prompts for the value. Automation should supply secrets through standard input, keeping them out of command-line arguments, logs, and chat. Configure the other variables from the template:

| Variable | Value / source |
| --- | --- |
| `PORT` | `8080` |
| `MPGS_DATABASE_URL` | Session pooler on port `5432`, user `lobbytally_runtime.YOUR_PROJECT_REF`, with `sslmode=require` |
| `MPGS_SUPABASE_URL` | `https://YOUR_PROJECT_REF.supabase.co` |
| `MPGS_SUPABASE_SERVICE_ROLE_KEY` | Server-side legacy `service_role` key from API Keys |
| `MPGS_SUPABASE_AVATAR_BUCKET` | `avatars` |
| `MPGS_CRON_SECRET` | Independent random secret, at least 32 non-whitespace bytes |
| `MPGS_TRUST_PROXY_HEADERS` | `true` |
| `MPGS_RATE_LIMIT_ENABLED` | `true` |
| `MPGS_AI_PROVIDER` | `disabled` until an AI provider is configured |
| Worker limits | Keep the template's 45-second budget and batch-size defaults initially |

The backend role password is encrypted in Vault as `lobbytally_database_password`. Retrieve it through an administrative dashboard or CLI session and write it directly to private configuration; there is no need to reset the project's `postgres` password. Obtain the actual hostname from **Connect → Session pooler**, rather than deriving it from the region. Percent-encode special characters in the password. Use port `5432`: the driver relies on session settings and prepared statements.

The database client adds the [official Supabase root certificate](https://supabase-downloads.s3-ap-southeast-1.amazonaws.com/prod/ssl/prod-ca-2021.crt) for official database / pooler hosts, retaining system roots, chain validation, and hostname validation. The bundled file is `crates/storage/certs/supabase-prod-ca-2021.crt`; its SHA-256 is `700723581420dd1ac98fd7e9ac529f0ef210eadcaf87fc868a3ad7d114c2f3b7`.

`MPGS_ADMIN_TOKEN` optionally enables administrative API access. `MPGS_STEAM_WEB_API_KEY` optionally enables official catalog synchronization; Store candidate discovery works without it. Optional AI settings are in the template. These variables are server-side and must not use a `VITE_` prefix. Preview deployments need their own Supabase project and credentials; access protection alone does not isolate database writes.

## 3. Build, verify, and promote

Use **Standard Protection** in Deployment Protection so production domains are public and previews remain protected. Review the source upload manifest before deploying:

```text
npx --yes vercel@64.5.0 deploy --dry --json --scope YOUR_VERCEL_TEAM
npx --yes vercel@64.5.0 deploy --prod --skip-domain --scope YOUR_VERCEL_TEAM
npx --yes vercel@64.5.0 inspect YOUR_DEPLOYMENT_ID --json --scope YOUR_VERCEL_TEAM
```

The manifest must include Rust / Web build inputs, Supabase SQL, and the bundled CA, and exclude `.tmp/`, environment files, databases, `node_modules/`, and `target/`. A dry run validates upload inputs; it does not replace a real build.

Confirm `readyState: READY` and check `/health/ready` on that deployment. For protected deployments, use the authenticated CLI's `vercel curl /health/ready --deployment YOUR_DEPLOYMENT_ID`. Require application JSON containing `status: "ready"` and `database: "ok"`; a login-page redirect ending in HTTP `200` is not a successful health check. Then promote:

```text
npx --yes vercel@64.5.0 promote YOUR_DEPLOYMENT_ID --scope YOUR_VERCEL_TEAM
```

Verify the public production URL again without a platform bypass token. The application HTTP deadline is 60 seconds, ingestion defaults to a 45-second budget, and avatar cleanup has its own 45-second deadline.

For a custom domain, add it to the Vercel project, use the returned TXT verification value and CNAME target, and select DNS only in Cloudflare. Verify HTTPS and the API before storing that origin in Vault. Do not reuse another installation's verification token.

## 4. Enable scheduled ingestion

Create these named secrets in Supabase Vault:

| Name | Value |
| --- | --- |
| `lobbytally_site_origin` | Verified public production HTTPS origin, without an API path |
| `lobbytally_cron_secret` | Exactly the same value as Vercel's `MPGS_CRON_SECRET` |

First call `POST /internal/v1/maintenance` with JSON body `{}` and `Authorization: Bearer <secret>`, then inspect the batch result. Run the schedule SQL through the SQL Editor or CLI:

```text
npx --yes supabase@2.121.0 db query --linked --file supabase/enable-schedules.sql --output json
```

Maintenance runs every minute; avatar cleanup runs every five minutes. Cursors, cooldowns, and leases live in PostgreSQL, so duplicate deliveries cannot start overlapping ingestion batches. Verify actual HTTP responses and durable progress, not just Cron enqueue success:

```sql
SELECT jobname, schedule, active FROM cron.job
WHERE jobname LIKE 'lobbytally-%';

SELECT id, status_code, timed_out, error_msg, created
FROM net._http_response ORDER BY created DESC LIMIT 20;

SELECT cursor_key, updated_at_ms FROM mpgs.source_cursors
ORDER BY updated_at_ms DESC LIMIT 10;
```

Run `supabase/disable-schedules.sql` to stop scheduling. Update the Vault origin when changing domains. When rotating the Cron secret, update both Vault and Vercel, then redeploy.

## 5. Update an existing installation

Check the linked projects and migration history, apply only new migrations, then follow “build → READY → health check → promote.” Environment changes require a new deployment. Database changes must remain compatible with the previous application version during the transition; rolling back a container does not undo database migrations.

Acceptance covers health, frontend assets, registration / login, saved preferences, search / details, votes, avatars, and continued ingestion progress. An initialized empty catalog can be ready while data is still being collected. After an avatar deletion succeeds, also confirm the Storage object was removed or its cleanup job completed. Stop any previous deployment only within the user's authorized migration scope.

## Local development and CI

`deploy/cloud.env` uses unquoted `KEY=value` lines. The server does not load it automatically; explicitly export it into the process environment, for example in PowerShell:

```powershell
$ErrorActionPreference = 'Stop'
foreach ($line in Get-Content -LiteralPath 'deploy/cloud.env' -Encoding UTF8) {
    if ($line -match '^([A-Z][A-Z0-9_]*)=(.*)$') {
        [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
    }
}
```

For local development, use a separate PostgreSQL 17+ database containing `pg_trgm`, with a development role allowed to create tables. Use development Supabase Storage for avatar functionality. Run `cargo run -p mpgs-dbtool --locked -- migrate`, `cargo run -p mpgs-server --locked`, and `pnpm web:dev` in the appropriate terminals.

Set a separate disposable `MPGS_TEST_DATABASE_URL`, for example `postgresql://postgres:password@127.0.0.1:5432/lobbytally_test?sslmode=disable`. Tests create and remove schemas; desktop E2E also needs a local role able to create temporary databases. For code changes, the standard checks are:

```text
cargo fmt --all -- --check
cargo test --workspace --locked -- --test-threads=4
cargo clippy --workspace --all-targets --locked -- -D warnings
pnpm web:test
pnpm web:build
```

For documentation-only changes, review diffs and links. GitHub CI skips Web, Rust, Linux, and container builds for recognized documentation-only diffs; code, configuration, SQL, or unknown files still run normal checks. Manual `full=true` runs and `v*` tags include desktop / multi-platform checks. `CI result` always aggregates outcomes. This policy controls GitHub Actions only; separately connected Vercel Git deployments follow the Vercel project's trigger settings.

Platform references: [Vercel Container Runtime](https://vercel.com/docs/functions/runtimes/container), [Services configuration](https://vercel.com/docs/services/config-reference), [Supabase database connections](https://supabase.com/docs/guides/database/connecting-to-postgres), [CLI migrations](https://supabase.com/docs/reference/cli/supabase-db-push), and [scheduled HTTP calls](https://supabase.com/docs/guides/functions/schedule-functions).
