# MPGS Server Docker Compose Deployment

This deployment target runs `mpgs-server` with Postgres and an optional Caddy reverse proxy.

## Build Locally

Build the service image on a local development machine or CI runner. Do not build on the VPS.

```bash
docker build -f Dockerfile.mpgs-server -t mpgs-server:local .
docker save mpgs-server:local -o mpgs-server-local.tar
```

Upload `mpgs-server-local.tar` plus the `deploy/` directory to the server, then load the image there:

```bash
docker load -i mpgs-server-local.tar
```

## Configure

Create a server-side `.env` from the example:

```bash
cp deploy/mpgs-server.env.example deploy/.env
```

Set at least:

- `POSTGRES_PASSWORD`
- `CADDY_DOMAIN` if using the Caddy profile

The Compose `.env` only locates the config directory indirectly through the compose file and sets container-level values such as the Postgres container credentials. Service settings and service secrets live in TOML files under `deploy/config`.

For first-run browser setup, create a setup token config:

```bash
cp deploy/config/setup.toml.example deploy/config/setup.toml
```

Set `setup.token_hash` to a SHA-256 hash in the format expected by `mpgs-server`:

```text
sha256:<base64-no-padding sha256(setup-token)>
```

The setup token allows the first-run setup API to write `deploy/config/active/service.toml` and `deploy/config/active/secrets.toml`. After setup writes active config, normal management access must use the admin token, not the setup token.

Do not put the raw setup token in `.env`, Postgres, docs, or logs.

Management configuration changes are written under `deploy/config/pending` first and report `restartRequired=true`. The service does not copy active secrets into pending files for service identity edits, so saving non-secret settings must not clear Steam, LLM, R2, or admin credentials. On the next startup, the service validates pending service configuration before promoting it to active. A later managed-restart slice will add the authenticated self-exit endpoint that triggers the container restart.

For manual offline configuration instead, create the active secrets file:

```bash
cp deploy/config/active/secrets.toml.example deploy/config/active/secrets.toml
```

Edit:

- `deploy/config/active/service.toml` for non-sensitive service identity and bind settings.
- `deploy/config/active/secrets.toml` for the Postgres URL, admin token hash, session secret, and future server-side secrets.

For the default Compose network, `deploy/config/active/secrets.toml` should use:

```toml
[database]
url = "postgres://mpgs:change-this-postgres-password@postgres:5432/mpgs"
```

Keep the database password in `deploy/.env` and `deploy/config/active/secrets.toml` in sync.

Set `admin.token_hash` to a SHA-256 hash in the format expected by `mpgs-server`:

```text
sha256:<base64-no-padding sha256(admin-token)>
```

Set `admin.session_secret` to a long random value used to sign short-lived admin session cookies. Do not put the raw admin token in `.env`, Postgres, docs, or logs.

Do not put Steam, LLM, R2, setup token, or admin token secrets in Postgres.

## Start Without Public HTTPS

This binds the service only to localhost on the host, which is suitable behind another reverse proxy:

```bash
docker compose --env-file deploy/.env -f deploy/compose.yml up -d
curl http://127.0.0.1:4310/healthz
curl http://127.0.0.1:4310/api/v1/service-info
```

## Start With Caddy

Point DNS for `CADDY_DOMAIN` at the server first. Then run:

```bash
docker compose --env-file deploy/.env \
  -f deploy/compose.yml \
  -f deploy/compose.caddy.yml \
  --profile caddy \
  up -d
```

The public client should use HTTPS:

```bash
curl https://$CADDY_DOMAIN/healthz
curl https://$CADDY_DOMAIN/api/v1/service-info
```

## Operational Boundary

- The VPS only runs `docker load` and `docker compose up`; it must not compile Rust or build the image.
- The service container runs database migrations on startup.
- The mounted config directory is writable so setup and later management APIs can write TOML configuration files; `.env` still must not contain service secrets.
- Public `/healthz` is intentionally minimal and does not expose configuration or secrets.
- Empty public catalog state is healthy; public library population belongs to later discovery/admin slices.
