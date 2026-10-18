# Agent deployment runbook

This runbook covers LobbyTally on **Vercel Container Runtime + Supabase**. Read the [provider setup guide](VERCEL_SUPABASE.en.md), or its [Chinese version](VERCEL_SUPABASE.md), for initial account and environment configuration. The current implementation serves the Rust API and built React frontend from one container; Supabase holds PostgreSQL data, private avatar objects, Vault secrets, and Cron schedules.

## Establish scope and the intended target

| User intent | Required target selection |
| --- | --- |
| Deploy a fresh clone or a new installation | Identify the user's intended Vercel team, Supabase organization, project names, regions, and domain. Create or link those targets within the user's authorization. Do not reuse this repository's recorded live project IDs. |
| Update the existing LobbyTally production deployment | Read the latest cloud section of [PRODUCTION_STATUS.md](PRODUCTION_STATUS.md), then verify that the linked provider projects and authenticated accounts match that deployment. Historical sections describe retired infrastructure. |
| Prepare deployment instructions or configuration | Complete the local changes and relevant review. Do not publish, run remote migrations, or change DNS unless the current request also authorizes those actions. |

Record the exact Vercel team/project and Supabase organization/project reference before external mutations. Check both local link state and provider ownership; matching project names alone are insufficient. If the intended organization, target, or authority is unclear, ask a focused question while completing independent local preparation.

A direct deployment request can authorize the necessary deployment work. This file does not grant permanent permission to deploy, push, commit, overwrite data, rotate credentials, or change DNS. Respect the current task's scope and existing authorization. Do not ask again for an action the user already authorized.

Use the Vercel/Supabase path throughout. Legacy `deploy/` VPS scripts, GHCR publishing, old SQLite migrations, and historical operations documents are not instructions to recreate a server. A fresh installation starts with its own data unless the user separately requests a data import.

## Reuse authentication and inspect the installed CLIs

The migration was verified with **Supabase CLI 2.121.0** and **Vercel CLI 64.5.0**. These versions document working examples, not a reason to ignore a newer installed version. Check the actual version and the relevant subcommand's `--help` before relying on flags or API conventions.

Reuse an existing authenticated CLI session or connected provider tool. Inspect account identity, memberships, project ownership, and link state without printing token files. If authentication is missing or expired, have the human complete the provider's interactive login/OAuth flow, then resume. Do not request passwords, access tokens, MFA codes, or recovery codes in chat, and do not bypass the login flow.

An existing Vercel session may use a non-default `--global-config` directory. Reuse that directory when needed; do not copy it into the repository or expose its contents. In the verified CLI, passing global configuration options through `vercel curl` caused them to reach the underlying curl command. Prefer a supported provider request or a direct HTTP request with credentials held in memory; do not disable Deployment Protection to work around this forwarding issue.

Verify `.vercel/project.json` against the intended team/project and the linked Supabase project against the intended project reference. Local link files are routing state, not proof of ownership or authorization.

## Handle environment values explicitly

[deploy/cloud.env.example](../deploy/cloud.env.example) defines the server-side settings. `deploy/cloud.env` is ignored by Git and excluded from the Vercel and Docker build uploads. Preserve those exclusions.

The Rust server and dbtool do **not** automatically load `deploy/cloud.env`. It is a simple literal `KEY=VALUE` file, not an executable shell script or a general dotenv implementation. For a local command that needs these values, explicitly parse them into that command's process environment. For example, from a dedicated PowerShell process at the repository root:

```powershell
$ErrorActionPreference = 'Stop'
foreach ($line in Get-Content -LiteralPath 'deploy/cloud.env' -Encoding UTF8) {
    if ([string]::IsNullOrWhiteSpace($line) -or $line.TrimStart().StartsWith('#')) {
        continue
    }
    if ($line -notmatch '^([A-Z][A-Z0-9_]*)=(.*)$') {
        throw 'Malformed environment assignment'
    }
    $settingName = $Matches[1]
    $settingValue = $Matches[2]
    if ($settingName -ne 'PORT' -and -not $settingName.StartsWith('MPGS_')) {
        throw 'Unexpected environment variable'
    }
    [Environment]::SetEnvironmentVariable($settingName, $settingValue, 'Process')
}
```

Values are literal: do not add shell quotes, interpolate expressions, or use `Invoke-Expression`/dot-sourcing. The parser produces no output. Load secrets only for the operation that needs them; avoid environment dumps, command tracing, raw provider responses containing credentials, or secret-bearing command arguments.

Copy values to the intended Vercel environment using the current CLI's supported secret input mechanism or the provider interface. Never commit or print the database URL/password, service role key, admin token, Cron secret, protection-bypass token, or CLI auth state. Keep all of them server-side; none belongs in a `VITE_` variable. Do not change persistent global environment variables to deploy this project.

For production, `PORT=8080` must agree with the container and routing configuration. The app also needs `MPGS_DATABASE_URL`, `MPGS_SUPABASE_URL`, `MPGS_SUPABASE_SERVICE_ROLE_KEY`, and `MPGS_CRON_SECRET`. Keep the Cron secret distinct from `MPGS_ADMIN_TOKEN`. Leave AI disabled until a provider is intentionally configured.

Preview deployments need an isolated Supabase project or an approved isolated environment with separate credentials, storage, and schedules. Do not copy production secrets into Preview or let a preview worker write production data.

## Apply migrations with accurate history

For a fresh Supabase project, use the migration files in [supabase/migrations](../supabase/migrations/) in filename order. The initial cloud setup currently comprises the application baseline, platform resources, PostgreSQL hardening, and restricted runtime access. The provider guide lists the exact files and setup sequence.

Prefer a linked CLI migration flow from the first application. Check the intended project and migration history before `db push`. The verified dry check is:

```text
npx --yes supabase@2.121.0 db push --linked --skip-vault --dry-run
```

Inspect current help before use. `--skip-vault` skips the CLI's Vault synchronization from `config.toml`; it does **not** suppress Vault operations inside migration SQL. The runtime-access migration still creates the encrypted database password secret.

If an operator already applied the SQL in SQL Editor, inspect the resulting schema and objects before recording those exact versions as applied with `migration repair`. Repair changes migration history; it does not execute SQL. Never mark a failed or unexecuted migration applied merely to unblock a push. Do not replay the fresh baseline or runtime-role creation against an existing production database. Add a reviewed forward migration for a production change.

Supabase migration history and `mpgs.schema_migrations` serve different purposes. The current application schema version remains `1`; it is not the count of Supabase migration files. Production startup checks readiness and does not run migrations or populate demonstration games.

The database connection must use:

- The dedicated `lobbytally_runtime` role, whose generated password is stored in Vault as `lobbytally_database_password`; do not distribute the project's administrator password to Vercel.
- The Session pooler host copied from the target project's Connect panel, port `5432`, and username `lobbytally_runtime.PROJECT_REF` using that target's project reference.
- A percent-encoded password in the connection URL and `sslmode=require`.
- The bundled official Supabase CA in [crates/storage/certs](../crates/storage/certs/), alongside system roots, with certificate-chain and hostname verification preserved.

Port `6543` transaction pooling is incompatible with the current driver's session settings and prepared statements. A TLS failure requires checking the actual host, official CA, and chain; never disable validation or trust a certificate copied from an unverified peer.

Keep `mpgs` out of the Data API's exposed schemas and the `avatars` bucket private. The backend role has explicit grants and RLS policies, not `BYPASSRLS` or database-owner privileges. New tables/functions in forward migrations need their intended grants and policies reviewed explicitly. Preserve the restriction preventing the runtime role from dispatching scheduled HTTP requests. Application account/session behavior remains in the existing API; this deployment does not switch users to Supabase Auth.

## Prepare the Vercel source upload and container configuration

The Vercel project root is the repository root, not `web`. [Dockerfile.vercel](../Dockerfile.vercel) builds both components. [vercel.json](../vercel.json) declares the service and its execution budget; the function limit is nested under the service:

```json
{
  "services": {
    "app": {
      "root": ".",
      "entrypoint": "Dockerfile.vercel",
      "runtime": "container",
      "functions": {
        "Dockerfile.vercel": { "maxDuration": 120 }
      }
    }
  }
}
```

Do not move this service's function configuration to a top-level `functions` field. Preserve the actual regions, rewrites, headers, and other settings in the complete file. The present deployment uses Tokyo; select an appropriate region for a new target rather than inheriting a recorded production location blindly. Confirm that the intended Vercel account supports the container runtime before changing a live domain.

[.vercelignore](../.vercelignore) is an upload allowlist. Review it against every `COPY` input in the Dockerfile, including Rust crates, migrations embedded by `include_str!`, the CA asset, workspace manifests, and frontend inputs. Keep `deploy/cloud.env`, other secrets, `.tmp`, `.vercel`, databases, dependency directories, and build outputs out of the upload. Review [.dockerignore](../.dockerignore) too.

The verified CLI command for the upload inventory is `--dry`, not `--dry-run`:

```powershell
npx --yes vercel@64.5.0 deploy --dry --json --scope $VercelScope --non-interactive --no-color
```

Capture the JSON manifest in an ignored local location if useful. Inspect `files[].path`, `fileCount`, and `totalSize`; check for missing build inputs and accidental private files. A successful dry inventory is **not** complete deployment-config validation and does not prove the container can build.

## Build, verify, and promote the exact deployment

When production deployment is authorized, stage a production build without changing domain aliases. The verified CLI flags are:

```powershell
npx --yes vercel@64.5.0 deploy --prod --skip-domain --no-wait --json --yes --scope $VercelScope
```

Supply a non-default existing auth directory only when needed. Capture the exact deployment ID and URL from this operation. Do not accidentally inspect or promote a different deployment because it has the same project name.

Use the supported inspect/status interface to wait for that deployment's actual `readyState` to become `READY`. With the verified CLI, `inspect --wait --timeout 50s --json` could finish with exit code `0` while still timed out; neither the exit code nor an uploaded-source message proves readiness. Inspect the returned state and the relevant build/runtime logs.

Test the deployment URL before promotion. It may still be protected even though it was built for Production. Reuse an existing authorized automation-bypass mechanism in request headers without printing it. Require an application JSON response from `/health/ready` with `database: "ok"`, and check `/health/live`, `/v1/meta`, the page, and a built static asset. An HTML sign-in page or a redirect followed by an eventual HTTP `200` is not an application health result.

If a check fails, keep the existing production alias on its prior deployment and fix the concrete problem. Do not disable TLS, authentication, RLS, or Deployment Protection to make a test pass.

Promote only the verified deployment, using the current CLI's `promote` help for the supported invocation. Then repeat the production-origin checks **without** a protection-bypass header. Require public application JSON and successful HTTPS, not just status `200`. Standard Protection should preserve authentication for previews while allowing the intended production site to be public.

When behavior changes warrant functional checks, use task-scoped test accounts/data and remove those fixtures afterward. Report actual results and any unavailable optional feature, such as an unconfigured AI provider. Record the verified deployment and current evidence in the latest section of `PRODUCTION_STATUS.md`; do not present old snapshot counts as current measurements.

## Initialize or update Vault and Cron

Enable schedules only after the intended production origin returns public application health JSON. Verify that an unauthenticated call to an internal endpoint is rejected and that an authorized call can complete its bounded work.

Vault contains:

| Secret name | Required value |
| --- | --- |
| `lobbytally_site_origin` | The verified public production HTTPS origin, without an API path. |
| `lobbytally_cron_secret` | Exactly the production `MPGS_CRON_SECRET`. |
| `lobbytally_database_password` | The restricted runtime-role password created by the access migration. Preserve it during an ordinary app update. |

For the first two secrets, create a missing named secret with the supported Vault API, or look up its ID and update that existing secret. Do not create duplicate names or print decrypted values. Transfer secret values through a secure supported input path; if a temporary SQL file is unavoidable, keep it in an ignored private location, avoid echoing its content, and remove it after the operation. Do not insert literal secrets into tracked migrations.

Run [supabase/enable-schedules.sql](../supabase/enable-schedules.sql) for the authorized initial schedule setup. It maintains named jobs for `POST /internal/v1/maintenance` every minute and `POST /internal/v1/avatar-cleanup` every five minutes. Existing named jobs are updated in place. Routine code updates do not require deleting/recreating schedules or rotating secrets.

A changed production domain requires updating `lobbytally_site_origin` after that origin is verified. A Cron-secret rotation requires matching Vault and Vercel values plus a deployment using the new environment. Use [supabase/disable-schedules.sql](../supabase/disable-schedules.sql) only when stopping these schedules is within the task's scope.

Inspect scheduling and actual delivery separately:

```sql
SELECT jobname, schedule, active
FROM cron.job WHERE jobname LIKE 'lobbytally-%';

SELECT jobid, status, return_message, start_time, end_time
FROM cron.job_run_details ORDER BY start_time DESC LIMIT 20;

SELECT id, status_code, timed_out, error_msg, created
FROM net._http_response ORDER BY created DESC LIMIT 20;
```

Cron success can mean only that `pg_net` queued a request. Confirm successful HTTP responses and inspect stored source cursors, task states, and observation timestamps across separate batches to establish real progress. Lease conflicts, dead jobs, or HTTP failures need diagnosis; do not reset live leases, raise retry limits, or weaken constraints merely to obtain a green response.

## Verify a custom domain using provider-supplied targets

Only change a domain/zone covered by the current authorization. Confirm the domain's ownership and the intended Vercel project before touching DNS. Obtain the current TXT verification value and CNAME target from Vercel for that exact domain/project; do not reuse the historical records in `PRODUCTION_STATUS.md`, invent a generic target, or copy values from another installation.

Apply only the required records in the authorized DNS zone, preserving unrelated records and services. Verify authoritative DNS answers, Vercel's domain verification/configuration state, and HTTPS for the requested hostname. Cached local resolution alone does not establish whether the authoritative change succeeded. If necessary, test the hostname against an independently verified current Vercel address while retaining hostname and certificate verification.

Check public JSON health on the final hostname, update Vault's site origin, and confirm an automatic scheduled request reaches it. Report domain/HTTPS verification separately from deployment readiness.

## Match validation and handoff to the change

| Change | Appropriate evidence |
| --- | --- |
| Markdown or agent instructions only | Diff review, local file/link checks, command/config consistency. Do not run Rust, frontend, database, or end-to-end suites solely for prose. |
| Backend behavior or SQL | Focused affected tests with an expendable PostgreSQL database, plus required project checks. Never set `MPGS_TEST_DATABASE_URL` to production. |
| UI behavior | Relevant frontend checks and inspection of the affected interaction/layout. |
| Deployment configuration or build inputs | Upload inventory, actual container build/state, protected deployment checks, then public checks after authorized promotion. |

The always-running `CI routing` job reports `run_checks`, `run_full`, and a reason. Documentation-only changes skip heavy jobs. A manual run with `full=false` selects normal checks; `full=true` and `v*` release tags select the full route. The stable final `CI result` summarizes the selected route and its results. Do not force a full run merely because a job was intentionally skipped.

Required-status settings are remote repository configuration, independent of these workflow names. Existing branch protection is not changed by this documentation or CI update. If the user later asks to configure required checks, consider the stable `CI result` rather than conditional heavy jobs; do not change those settings without authorization.

The handoff should state what changed, the selected target, the exact checks actually performed, the resulting deployment/domain/schedule state when applicable, and any remaining action. If external authorization is missing, finish and present the local preparation, then ask only for the specific missing authorization. Do not claim a deployment, migration, or public endpoint was verified from a dry run or source review.
