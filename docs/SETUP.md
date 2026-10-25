# Setup — local development

This guide gets Table Explorer running on macOS or Linux with a project-local PostgreSQL that
never touches any database you already have. For containers see
[DOCKER_DEPLOYMENT.md](DOCKER_DEPLOYMENT.md).

## 1. Prerequisites

| Tool | Version | Install |
|---|---|---|
| Rust (rustup) | stable (tested 1.99) | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| WASM target | — | `rustup target add wasm32-unknown-unknown` |
| cargo-leptos | 0.3.11 | `cargo install cargo-leptos --version 0.3.11 --locked` (a few minutes) |
| PostgreSQL | 13+ (tested 14) | macOS: `brew install postgresql@14` · Debian/Ubuntu: `sudo apt-get install postgresql` |

cargo-leptos downloads the matching `wasm-bindgen` CLI by itself on the first build.

## 2. Database

```bash
git clone https://github.com/giridharmb/TableExplorer.git
cd TableExplorer

# Project-local cluster on port 5433, data in ./pgdata (git-ignored)
initdb -D ./pgdata -U postgres --auth=trust     # Linux: /usr/lib/postgresql/<ver>/bin/initdb
pg_ctl -D ./pgdata -o "-p 5433 -k /tmp" -l pgdata/log.txt start
createdb -h localhost -p 5433 -U postgres webui
psql -h localhost -p 5433 -U postgres -d webui -f db/seed.sql
psql -h localhost -p 5433 -U postgres -d webui -c "select count(*) from customers"   # 1000
```

Stop it later with `pg_ctl -D ./pgdata stop`.

Using an existing server instead? Export the connection strings; `config/app.toml` reads them:

```bash
export DATABASE_URL="host=db.example.com port=5432 user=reader password=… dbname=analytics"
export STORAGE_DATABASE_URL="host=db.example.com port=5432 user=app password=… dbname=analytics"
```

The storage database needs PostgreSQL 13+ (`gen_random_uuid()`); the app creates its
`saved_queries` table on startup.

## 3. Run

```bash
cargo leptos watch          # http://127.0.0.1:3000 — rebuilds and reloads the browser on save
```

Expected log:

```
source "local_pg" (postgres) ready
source "bq_demo" (bigquery) unavailable: no GCP credentials: …
listening on http://127.0.0.1:3000
```

The BigQuery demo source shows as *unavailable* in the table picker until it is configured
(section 5) — or set `enabled = false` on it in `config/app.toml`.

## 4. Test

```bash
cargo test --features ssr --lib                                   # 13 unit tests
cargo check --lib --features hydrate --target wasm32-unknown-unknown   # the browser bundle
cargo leptos build --release                                      # what Docker builds
```

Manual acceptance checks with expected row counts are in
[AI_AGENT_INSTRUCTIONS.md §14](../AI_AGENT_INSTRUCTIONS.md#14-verification-checklist-run-all-of-these).

## 5. Configuration

Everything lives in [`config/app.toml`](../config/app.toml) (override the path with `APP_CONFIG`).
Values may use `${VAR}` or `${VAR:-default}`. Restart after editing.

```toml
[storage]
postgres_url = "${STORAGE_DATABASE_URL:-host=localhost port=5433 user=postgres dbname=webui}"

[[source]]                      # one block per data source → one group in the table picker
id = "local_pg"                 # a-z 0-9 _ -   (used in links and saved searches)
label = "Local PostgreSQL"
kind = "postgres"
url = "${DATABASE_URL:-host=localhost port=5433 user=postgres dbname=webui}"
schema = "public"
tables = ["customers", "sensor_readings"]   # omit / [] = every table in the schema
statement_timeout_ms = 5000
max_export_rows = 100000

[[source]]
id = "bq_sales"
label = "Sales (BigQuery)"
kind = "bigquery"
project = "my-gcp-project"
dataset = "sales"
location = "US"
tables = []                     # every table in the dataset
```

All keys and defaults: [AI_AGENT_INSTRUCTIONS.md §7](../AI_AGENT_INSTRUCTIONS.md#7-configuration-configapptoml-override-path-with-app_config).

### BigQuery credentials

The BigQuery adapter uses Application Default Credentials, in this order:
`GOOGLE_APPLICATION_CREDENTIALS=/path/key.json` → `gcloud auth application-default login` →
the GCP metadata server (Cloud Run, GKE, GCE). The identity needs **BigQuery Data Viewer** on
the dataset and **BigQuery Job User** on the project.

For a credential-free local test, run the BigQuery emulator and point the source at it:

```bash
docker run -p 9050:9050 ghcr.io/goccy/bigquery-emulator:latest --project=test --dataset=demo
```

```toml
endpoint = "http://localhost:9050"
auth = "none"
project = "test"
dataset = "demo"
```

## 6. Troubleshooting

| Symptom | Fix |
|---|---|
| `cannot load config: … environment variable X is not set` | export `X`, or give it a default: `${X:-value}` |
| `source "…" unavailable: cannot connect` | database not running / wrong URL; check `pg_ctl status` and the connection string |
| `Reload TCP port 127.0.0.1:3001 already in use` | another `cargo leptos watch` is running; stop it |
| `queries overflow the depth limit` in a release build | keep `#![recursion_limit = "256"]` at the top of `src/lib.rs` |
| Page loads but buttons do nothing, console shows "hydration error" | server and client rendered different HTML; see the Leptos gotchas in [AI_AGENT_INSTRUCTIONS.md §11](../AI_AGENT_INSTRUCTIONS.md#11-leptos-gotchas-each-one-cost-real-debugging-time--do-not-regress) |
| Saved searches "disabled" | `[storage] postgres_url` unreachable or PostgreSQL < 13 |
