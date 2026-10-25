# Docker deployment

The repository ships a multi-stage [`Dockerfile`](../Dockerfile) (Rust builder → `debian:bookworm-slim`
runtime, non-root) and a [`docker-compose.yml`](../docker-compose.yml) that also starts a seeded
PostgreSQL.

## 1. Quick start (app + database)

```bash
git clone https://github.com/giridharmb/TableExplorer.git
cd TableExplorer
docker compose up --build        # first build: ~5–10 min (cargo-leptos + release build)
open http://localhost:3000
```

What happens:

1. `db` (postgres:16) starts with user/password `postgres`/`postgres`, database `webui`, and runs
   [`db/seed.sql`](../db/seed.sql) once (first start with an empty volume) → tables `customers`
   and `sensor_readings`, 1000 rows each.
2. `app` waits for the database health check, then starts with
   `DATABASE_URL` / `STORAGE_DATABASE_URL` pointing at `db`. It creates `saved_queries` itself.
3. The BigQuery demo source is listed as *unavailable* until you add credentials (section 4).

Useful commands:

```bash
docker compose logs -f app                 # "source … ready", "listening on http://0.0.0.0:3000"
docker compose down                        # stop (data kept in the pgdata volume)
docker compose down -v                     # stop and delete the database volume (re-seeds next time)
psql -h localhost -p 5433 -U postgres webui   # the compose database from the host (password: postgres)
```

## 2. Image only (bring your own database)

```bash
docker build -t table-explorer .

docker run --rm -p 3000:3000 \
  -e DATABASE_URL="host=db.internal port=5432 user=reader password=secret dbname=analytics" \
  -e STORAGE_DATABASE_URL="host=db.internal port=5432 user=app password=secret dbname=analytics" \
  table-explorer
```

On Docker Desktop, a database running on your host is reachable as `host.docker.internal`
(e.g. the project-local one: `host=host.docker.internal port=5433 user=postgres dbname=webui`).

## 3. What's in the image

| Path | Content |
|---|---|
| `/app/webui` | server binary (axum + Leptos SSR) |
| `/app/site/pkg/` | `webui.js`, `webui.wasm`, `webui.css`, `snippets/` (browser bundle) |
| `/app/config/app.toml` | data-source configuration (copied at build time) |

Runtime environment (set in the Dockerfile; override with `-e`):

| Variable | Default | Purpose |
|---|---|---|
| `LEPTOS_SITE_ADDR` | `0.0.0.0:3000` | listen address |
| `LEPTOS_OUTPUT_NAME` / `LEPTOS_SITE_ROOT` / `LEPTOS_SITE_PKG_DIR` | `webui` / `site` / `pkg` | where the browser bundle is served from — keep as is |
| `APP_CONFIG` | `/app/config/app.toml` | config file path |
| `DATABASE_URL` | (from config default) | PostgreSQL source connection string |
| `STORAGE_DATABASE_URL` | (from config default) | saved-searches database |
| `GOOGLE_APPLICATION_CREDENTIALS` | unset | path to a GCP service-account key inside the container |
| `GCP_PROJECT` | `my-gcp-project` | used by the sample BigQuery source |

The container runs as uid `10001`, exposes port `3000`, and needs no writable filesystem.

## 4. Changing sources without rebuilding

Mount your own config over the baked-in one:

```bash
docker run -p 3000:3000 \
  -v "$PWD/my-app.toml:/app/config/app.toml:ro" \
  -e DATABASE_URL=… -e STORAGE_DATABASE_URL=… \
  table-explorer
```

With compose, uncomment the `./config/app.toml:/app/config/app.toml:ro` volume in
`docker-compose.yml`. Restart the container after editing.

### BigQuery inside the container

```bash
mkdir -p secrets && cp ~/Downloads/my-sa-key.json secrets/gcp-key.json   # secrets/ is git-ignored
docker run -p 3000:3000 \
  -v "$PWD/secrets/gcp-key.json:/secrets/gcp-key.json:ro" \
  -e GOOGLE_APPLICATION_CREDENTIALS=/secrets/gcp-key.json \
  -e GCP_PROJECT=my-gcp-project \
  -e DATABASE_URL=… -e STORAGE_DATABASE_URL=… \
  table-explorer
```

The service account needs **BigQuery Data Viewer** (dataset) and **BigQuery Job User** (project).
On Cloud Run / GKE, attach the service account to the workload instead of mounting a key.

## 5. Production checklist

- **TLS & auth:** the app has no login. Put it behind a TLS-terminating reverse proxy with SSO
  (e.g. an identity-aware proxy, oauth2-proxy, Cloud Run + IAP).
- **Least privilege:** source DB users need only `SELECT` on exposed tables; the storage user needs
  `CREATE` (first start), `INSERT` and `SELECT` on `saved_queries`.
- **Postgres TLS:** connections are plain TCP. For managed databases that require TLS, keep the
  database on a private network or add `tokio-postgres-rustls` (see `src/server/postgres.rs`).
- **Health check:** `GET /` returns 200 when the server is up (it also runs a query against the default table).
- **Scaling:** stateless — run several replicas behind a load balancer; all state is in PostgreSQL.
- **Resources:** ~25 MB binary; memory grows with export size (exports are built in memory, capped by `max_export_rows`).

## 6. Cloud Run example

```bash
gcloud builds submit --tag "REGION-docker.pkg.dev/PROJECT/apps/table-explorer"
gcloud run deploy table-explorer \
  --image "REGION-docker.pkg.dev/PROJECT/apps/table-explorer" \
  --port 3000 --service-account table-explorer@PROJECT.iam.gserviceaccount.com \
  --set-env-vars "DATABASE_URL=host=/cloudsql/PROJECT:REGION:INSTANCE user=reader password=…,STORAGE_DATABASE_URL=…,GCP_PROJECT=PROJECT" \
  --add-cloudsql-instances PROJECT:REGION:INSTANCE \
  --no-allow-unauthenticated
```

(Cloud SQL's unix socket `host=/cloudsql/…` avoids the TLS requirement; `--no-allow-unauthenticated`
plus IAP provides authentication.)

## 7. Troubleshooting

| Symptom | Fix |
|---|---|
| Build fails with `queries overflow the depth limit` | `src/lib.rs` must keep `#![recursion_limit = "256"]` |
| `cannot load config: … environment variable … is not set` | pass it with `-e`, or give it a `${VAR:-default}` in the config |
| Page renders but is not interactive, 404 for `/pkg/webui.wasm` | `LEPTOS_SITE_ROOT`/`LEPTOS_SITE_PKG_DIR`/`LEPTOS_OUTPUT_NAME` changed — restore the defaults above |
| `source "local_pg" unavailable: cannot connect` | wrong `DATABASE_URL`; from a container, `localhost` is the container itself |
| Seed data missing in compose | the init script only runs on an empty volume: `docker compose down -v && docker compose up` |
