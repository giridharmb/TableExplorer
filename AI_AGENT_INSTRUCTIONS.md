# AI Agent Instructions — Table Explorer (Rust/WASM · PostgreSQL · BigQuery)

You are an AI coding agent. This document is everything you need to **set up, rebuild, verify and
extend** this application on a new machine. Read it end to end before starting.

Repository: <https://github.com/giridharmb/TableExplorer> · Companion docs:
[docs/SETUP.md](docs/SETUP.md) (local setup), [docs/USER_GUIDE.md](docs/USER_GUIDE.md) (search
builder), [docs/END_TO_END_FLOW.md](docs/END_TO_END_FLOW.md) (sequence diagrams),
[docs/DOCKER_DEPLOYMENT.md](docs/DOCKER_DEPLOYMENT.md) (containers).

- **Mode A — the repository is available** (`git clone https://github.com/giridharmb/TableExplorer.git`):
  follow §12 *Setup* (or §12.6 *Docker*) and §14 *Verify*.
  Use the rest as the reference for how things work and why.
- **Mode B — rebuild from scratch**: follow §16 *Build order*. Every file's contract is in
  §5–§11; the traits, schemas and SQL rules given verbatim are normative — reproduce them exactly.

Work rules for the agent:
1. Finish each build phase with its check (§16) before starting the next one. Never skip a check.
2. Compile both targets after every change to shared code:
   `cargo check --features ssr` **and** `cargo check --lib --features hydrate --target wasm32-unknown-unknown`.
3. Never interpolate user input into SQL. Values are always bound parameters; identifiers are
   only ever names that came from the database catalog/API and are quoted by the dialect.
4. When something contradicts this document, trust the running code and the tests, then fix the doc.

---

## 1. What the product does

A web app that browses and searches database tables **entirely server-side**:

| Feature | Behaviour |
|---|---|
| Pluggable sources | `config/app.toml` lists data sources (`kind = "postgres"` or `"bigquery"`) and the tables each exposes. The UI's table picker groups tables by source and shows a PostgreSQL/BigQuery badge. A source that fails to start (bad credentials, DB down) stays listed as *unavailable* with the reason, and the app keeps running. |
| Server-side paging | Only the current page (10/25/50/100/200 rows) is fetched and rendered. Total count, sort (click a header: asc → desc → none) and page all run in SQL (`COUNT(*)`, `ORDER BY … NULLS LAST`, `LIMIT/OFFSET`). |
| SSR + hydration | The first HTML response already contains the table (server-rendered), then the WASM bundle hydrates it and further interaction calls server functions. |
| Mixed schemas | Columns and types are introspected per table. Every cell is fetched as text (`NULL` preserved), so any column type works. Cells render with badges: `NULL`, `empty`, booleans (✓/✗), error-looking values (`#ERROR…`, `ERR_…`, `NaN`, `Infinity`, `-1`, JSON containing `"error"`) in red. |
| Search builder | Nested groups (any depth). Each group: AND/OR toggle + NOT checkbox, `+ Rule`, `+ Group`, remove. Each rule: column, operator, multiple values as chips (Enter adds; values are OR'ed), `Aa` case-sensitive toggle. Empty rules/groups are ignored. Rules and groups can be dragged (⠿ handle) to reorder or move into another group (never into itself/descendants). |
| Quick search | One box, ILIKE across every column (AND'ed with the filter tree). Matches highlighted in cells. |
| Operators | contains, equals, not equals, starts with, ends with, wildcard pattern (`*` any run, `?` one char), regex, not regex, `>`, `≥`, `<`, `≤` (value cast to the column's real type), is NULL, is not NULL, is empty/blank. `not equals`/`not regex` count NULL cells as "not equal". |
| Errors | Invalid regex, uncastable comparison value, unknown column etc. show a red "Query failed" box with the database's message. Queries are time-limited (Postgres `statement_timeout`, BigQuery `jobTimeoutMs`). |
| Shareable URL | The address bar always reflects the current search as `/?q=<url-encoded SearchRequest JSON>`; opening it server-renders the same search. Malformed `q` falls back to the default table. |
| Save & share | "🔗 Save & share" → name → stored in PostgreSQL table `saved_queries` with a UUID → link `/s/<uuid>` (copy button). Anyone opening it sees the same table/filters with a "📌 Saved search" banner. "Saved searches ▾" lists the 20 most recent. |
| Export | "⬇ Excel (.xlsx)" and "⬇ JSON" download **all** matching rows (ignoring paging), sorted like the UI, capped at the source's `max_export_rows` (header `x-export-truncated: true` when capped). XLSX: bold frozen header, autofilter, numeric columns written as numbers, a second "Search" sheet describing the filter. JSON: array of objects in column order; numeric/boolean columns typed. |
| Show SQL | Toggle shows the generated SQL and bound params for the current page. |

---

## 2. Tech stack (pinned versions that are known to work)

| Layer | Choice | Version |
|---|---|---|
| Language | Rust (edition 2021) | rustc 1.99 (any recent stable ≥ 1.85 should work) |
| UI framework | Leptos (signals, SSR + hydrate), leptos_router, leptos_meta | leptos 0.8.22, router 0.8.17, meta 0.8.7 |
| Server | axum + leptos_axum, tokio | axum 0.8.9, leptos_axum 0.8.10, tokio 1.54 |
| Build tool | cargo-leptos (builds server bin + WASM + CSS, dev watch/reload) | 0.3.11 |
| WASM glue | wasm-bindgen (cargo-leptos auto-installs the matching CLI) | 0.2.129 |
| PostgreSQL | tokio-postgres + deadpool-postgres (pool) | 0.7.18 / 0.14.2 |
| BigQuery | REST API v2 via reqwest (rustls) + gcp_auth (ADC) | reqwest 0.12.28, gcp_auth 0.12.7 |
| Excel | rust_xlsxwriter | 0.101.0 |
| Config | toml + serde | toml 1.1.8 |
| Async traits | async-trait (needed for `dyn DataSource`) | 0.1.92 |
| Database for storage | PostgreSQL **13+** (uses built-in `gen_random_uuid()`) | tested on 14.20 |

---

## 3. Architecture

```
 Browser (WASM, Leptos hydrate)                     Server (axum + Leptos SSR, tokio)
 ┌─────────────────────────────┐   HTTP   ┌──────────────────────────────────────────────┐
 │ App → Router                │ ───────► │ leptos_routes: SSR of "/" and "/s/:id"       │
 │  ├ Home   (/ , /?q=)        │          │ server fns  POST /api/<fn>  (JSON in/out)    │
 │  ├ Shared (/s/:id)          │          │ GET /export/{xlsx|json}?q=<json>             │
 │  └ Explorer (all UI state)  │ ◄─────── │                                              │
 │     GroupView/RuleView/...  │  HTML +  │  app.rs server fns ──► server::query         │
 └─────────────────────────────┘  JSON    │                         │ (compile + paginate)│
                                          │                         ▼                    │
                                          │                server::source registry       │
                                          │          (built by FACTORY from config)      │
                                          │            ┌────────────┴────────────┐       │
                                          │   PostgresSource+PgDialect  BigQuerySource+BqDialect
                                          │            │                         │       │
                                          │      deadpool → PostgreSQL     REST → BigQuery
                                          │  server::store → PostgreSQL (saved_queries)  │
                                          └──────────────────────────────────────────────┘
```

### Request flows

1. **First load `/`** — `Home` awaits `list_sources()` (inside `<Await>`), picks the `?q=` request or
   the first available table, renders `Explorer`; `Explorer`'s `results` Resource runs
   `run_search` on the server; the route uses `SsrMode::Async`, so the HTML is sent only once the
   rows are rendered. Resource results are serialized into the page for hydration.
2. **Hydration** — the WASM bundle replays the same components against the server HTML, reusing
   the serialized resource values (no second query), then attaches event handlers.
3. **Interaction** — any change to source/table/filter/quick/sort/page/page size updates a
   `request: Memo<SearchRequest>`; the `results` Resource re-runs `run_search` (POST
   `/api/run_search…`, JSON). A `<Transition>` keeps the old table visible (dimmed, progress bar)
   until the new page arrives. The address bar is rewritten with `history.replaceState`.
4. **Save** — `save_query(name, request)` validates the request by running it (page size 1),
   inserts into `saved_queries`, returns the row; UI shows `origin + /s/<uuid>` and replaces the URL.
5. **Open `/s/<uuid>`** — `Shared` awaits `(load_query(id), list_sources())` and renders `Explorer`
   with `init = saved.request` and the banner.
6. **Export** — a plain `<a href="/export/xlsx?q=…" download>`; the axum handler deserializes the
   request, runs `query::export` (no paging, `LIMIT max_export_rows+1`) and streams the file.

---

## 4. Design patterns used (and where)

| Pattern | Where | Purpose |
|---|---|---|
| **Factory** | `server::source::create(&SourceConfig) -> Arc<dyn DataSource>` | The **only** place mapping a config `kind` to an adapter. New engine = new match arm. |
| **Strategy** | `trait Dialect` (`PgDialect`, `BqDialect`) | All SQL differences (quoting, params, LIKE/regex, casts, text rendering, sortability) behind one interface; the compiler never branches on engine. |
| **Adapter** | `trait DataSource` (`PostgresSource`, `BigQuerySource`) | Wraps each client library (tokio-postgres pool, BigQuery REST) behind 5 primitives. |
| **Template method** | `server::query::{prepare, search, export}` | The search algorithm (validate table → columns → compile WHERE → count + page / export) is written once over `&dyn DataSource`. |
| **Registry** | `server::source::REGISTRY` (`OnceLock<Vec<Entry>>`) | Built once at startup from config; looked up by source id. Keeps failed sources with their error. |
| **Interpreter / compiler** | `query::build_where` | Walks the `Filter` tree producing SQL + a parameter list. |
| **Repository** | `server::store` | Persistence of saved searches behind `save/load/recent`. |

---

## 5. Repository layout

```
Cargo.toml                 crate "webui": lib (cdylib+rlib) + bin; features ssr / hydrate
config/app.toml            storage DB + [[source]] list (see §7)
db/seed.sql                mock data: customers, sensor_readings (1000 rows each)
style/main.css             all styling (dark default + light via prefers-color-scheme)
src/lib.rs                 #![recursion_limit = "256"]; mod app; mod model; #[cfg(ssr)] mod server; #[cfg(hydrate)] fn hydrate()
src/main.rs                (ssr) server bootstrap: server::init(), axum Router, /export route
src/model.rs               shared serde types (compiled for server AND wasm)
src/app.rs                 server fns + all UI components + UI-state tests
src/server/mod.rs          pub mods + init() (load config, build registry, open storage)
src/server/config.rs       AppConfig/SourceConfig/SourceKind, ${ENV} expansion, validation
src/server/source.rs       Dialect + DataSource traits, factory, registry, Entry helpers
src/server/query.rs        Params, build_where, prepare/search/export (+ dialect tests)
src/server/postgres.rs     pool(), db_err(), PostgresSource, PgDialect
src/server/bigquery.rs     BigQuerySource (REST), BqDialect, parse_rows, field_type
src/server/store.rs        saved_queries schema + save/load/recent
src/server/export.rs       axum handler, to_json, to_xlsx
Dockerfile                 multi-stage: rust:1-bookworm builder (cargo-leptos) → debian:bookworm-slim runtime
docker-compose.yml         postgres:16 (seeded via db/seed.sql) + app
.dockerignore / .gitignore target/, pgdata/, IDE files, secrets/ excluded
docs/                      SETUP, USER_GUIDE, END_TO_END_FLOW, DOCKER_DEPLOYMENT, templates/README_TEMPLATE.md, assets/
pgdata/                    (local only, git-ignored) project-local PostgreSQL data dir
```

---

## 6. Cargo.toml (normative)

```toml
[package]
name = "webui"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
leptos = "0.8"
leptos_meta = "0.8"
leptos_router = "0.8"
leptos_axum = { version = "0.8", optional = true }
axum = { version = "0.8", optional = true }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync"], optional = true }
tokio-postgres = { version = "0.7", optional = true }
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["preserve_order"] }   # keeps export column order
web-sys = { version = "0.3", features = ["DataTransfer", "History"] }
wasm-bindgen = { version = "0.2", optional = true }
console_error_panic_hook = { version = "0.1", optional = true }
async-trait = { version = "0.1.92", optional = true }
toml = { version = "1.1.8", optional = true }
deadpool-postgres = { version = "0.14.2", optional = true }
gcp_auth = { version = "0.12.7", optional = true }
rust_xlsxwriter = { version = "0.101.0", optional = true }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"], optional = true }

[features]
hydrate = ["leptos/hydrate", "dep:wasm-bindgen", "dep:console_error_panic_hook"]
ssr = [
    "dep:axum", "dep:tokio", "dep:tokio-postgres", "dep:deadpool-postgres", "dep:async-trait",
    "dep:toml", "dep:reqwest", "dep:gcp_auth", "dep:rust_xlsxwriter", "dep:leptos_axum",
    "leptos/ssr", "leptos_meta/ssr", "leptos_router/ssr",
]

[profile.wasm-release]
inherits = "release"
opt-level = 'z'
lto = true
codegen-units = 1
panic = "abort"

[package.metadata.leptos]
output-name = "webui"
site-root = "target/site"
site-pkg-dir = "pkg"
style-file = "style/main.css"
site-addr = "127.0.0.1:3000"
reload-port = 3001
bin-features = ["ssr"]
bin-default-features = false
lib-features = ["hydrate"]
lib-default-features = false
lib-profile-release = "wasm-release"
```

All server-only crates are `optional` and enabled by `ssr`, so they never get compiled to WASM.

---

## 7. Configuration (`config/app.toml`, override path with `APP_CONFIG`)

- Any value may contain `${VAR}` or `${VAR:-default}`; expansion happens before TOML parsing,
  **comment lines are skipped**, a missing variable without default is a startup error.
- Source `id` must match `[a-z0-9_-]+` and be unique (it appears in URLs and saved searches).
- The config is read at startup only — restart after editing.

| Key | Applies to | Default | Meaning |
|---|---|---|---|
| `[storage] postgres_url` | — | required | tokio-postgres connection string for `saved_queries` (key=value form, e.g. `host=… port=… user=… password=… dbname=…`) |
| `id`, `label` | every source | required | identifier / display name |
| `kind` | every source | required | `"postgres"` or `"bigquery"` |
| `tables` | every source | `[]` | tables/views to expose; empty = all in schema/dataset |
| `max_export_rows` | every source | `100000` | export cap |
| `enabled` | every source | `true` | `false` hides the source entirely |
| `url` | postgres | required | connection string |
| `schema` | postgres | `"public"` | schema to read |
| `statement_timeout_ms` | postgres | `30000` | applied via connection option `-c statement_timeout=…` |
| `pool_size` | postgres | `8` | deadpool max size |
| `project`, `dataset` | bigquery | required | GCP project id and dataset |
| `location` | bigquery | none | dataset location, e.g. `"US"`, `"europe-west2"` |
| `endpoint` | bigquery | `https://bigquery.googleapis.com` | set `http://localhost:9050` for the emulator |
| `auth` | bigquery | `"adc"` | `"adc"` = Application Default Credentials; `"none"` = no header (emulator) |
| `timeout_ms` | bigquery | `30000` | `timeoutMs` and `jobTimeoutMs` of each query |

Reference file:

```toml
[storage]
postgres_url = "${STORAGE_DATABASE_URL:-host=localhost port=5433 user=postgres dbname=webui}"

[[source]]
id = "local_pg"
label = "Local PostgreSQL"
kind = "postgres"
url = "${DATABASE_URL:-host=localhost port=5433 user=postgres dbname=webui}"
schema = "public"
tables = ["customers", "sensor_readings"]   # explicit, so saved_queries isn't exposed
statement_timeout_ms = 5000
pool_size = 8

[[source]]
id = "bq_demo"
label = "BigQuery demo"
kind = "bigquery"
enabled = true            # listed as "unavailable" until GCP credentials exist
project = "${GCP_PROJECT:-my-gcp-project}"
dataset = "demo"
location = "US"
tables = []
timeout_ms = 30000
max_export_rows = 50000
```

Implementation (`config.rs`): `AppConfig { storage: StorageConfig, #[serde(rename="source")] sources: Vec<SourceConfig> }`;
`SourceConfig { id, label, tables, max_export_rows, enabled, #[serde(flatten)] kind: SourceKind }`;
`#[serde(tag = "kind", rename_all = "lowercase")] enum SourceKind { Postgres {..}, Bigquery {..} }`
with `fn name(&self) -> &'static str`; `enum BqAuth { #[default] Adc, None }`.
`AppConfig::load()` reads the file; `AppConfig::parse(&str)` expands env + validates (unit-tested).

---

## 8. Shared data model (`src/model.rs`) — wire format

All types derive `Clone, Debug, PartialEq, Serialize, Deserialize` (Op/Join also `Copy, Eq`).

```rust
pub enum Join { And, Or }
pub enum Op { Contains, Equals, NotEquals, StartsWith, EndsWith, Wildcard, Regex, NotRegex,
              Gt, Gte, Lt, Lte, IsNull, NotNull, IsEmpty }
// Op::ALL (UI order), Op::label() (UI text), Op::key() = format!("{:?}"), Op::from_key(),
// Op::needs_value() = !matches!(IsNull | NotNull | IsEmpty)

pub enum Filter {
    Rule  { column: String, op: Op, values: Vec<String>, case_sensitive: bool }, // values OR'ed
    Group { join: Join, negate: bool, children: Vec<Filter> },
}
pub struct SearchRequest {
    pub source: String, pub table: String, pub filter: Filter, pub quick: String,
    pub sort: Option<(String, bool)>,   // (column, descending)
    pub page: u32, pub page_size: u32,
}   // SearchRequest::new(source, table): empty AND group, page 1, page_size 25
pub struct Column { pub name: String, pub ty: String }     // ty = engine type name
pub struct SearchPage { pub columns: Vec<Column>, pub rows: Vec<Vec<Option<String>>>,
    pub total: i64, pub page: u32, pub page_size: u32, pub sql: String, pub elapsed_ms: f64 }
pub struct SourceInfo { pub id: String, pub label: String, pub kind: String,
    pub tables: Vec<String>, pub error: Option<String> }
pub struct SavedQuery { pub id: String, pub name: String, pub created_at: String,
    pub request: SearchRequest }
```

Example `SearchRequest` JSON (this is exactly what goes in `?q=` and in `saved_queries.request`):

```json
{"source":"local_pg","table":"customers",
 "filter":{"Group":{"join":"And","negate":false,"children":[
   {"Rule":{"column":"active","op":"Equals","values":["true"],"case_sensitive":false}},
   {"Group":{"join":"Or","negate":false,"children":[
     {"Rule":{"column":"email","op":"Regex","values":["^(alice|bob)\\d+@"],"case_sensitive":false}},
     {"Rule":{"column":"notes","op":"IsNull","values":[],"case_sensitive":false}}]}}]}},
 "quick":"","sort":["balance",true],"page":1,"page_size":25}
```

---

## 9. Server side

### 9.1 Traits, factory and registry (`src/server/source.rs`) — normative

```rust
pub type Rows = Vec<Vec<Option<String>>>;

pub trait Dialect: Send + Sync {
    fn quote_ident(&self, name: &str) -> String;
    fn param(&self, n: usize) -> String;                 // n is 1-based; all params are strings
    fn text(&self, col: &Column) -> String;              // column rendered as text
    fn text_expr(&self, expr: &str) -> String;           // e.g. COUNT(*) as text
    fn like(&self, lhs: &str, pattern: &str, case_sensitive: bool) -> String;
    fn regex(&self, lhs: &str, pattern: &str, case_sensitive: bool) -> String;
    fn compare_type(&self, col: &Column) -> Option<String>;  // None = not comparable with < >
    fn sortable(&self, col: &Column) -> bool;            // false → ORDER BY text(col)
    fn equals(&self, lhs: &str, rhs: &str, cs: bool) -> String {   // default impl
        if cs { format!("{lhs} = {rhs}") } else { format!("LOWER({lhs}) = LOWER({rhs})") }
    }
}

#[async_trait]
pub trait DataSource: Send + Sync {
    fn dialect(&self) -> &dyn Dialect;
    fn table_ref(&self, table: &str) -> String;          // fully-qualified, quoted
    async fn list_tables(&self) -> Result<Vec<String>, String>;
    async fn columns(&self, table: &str) -> Result<Vec<Column>, String>;
    async fn query(&self, sql: &str, params: &[String]) -> Result<Rows, String>; // select list all text
}

pub async fn create(cfg: &SourceConfig) -> Result<Arc<dyn DataSource>, String> {
    Ok(match &cfg.kind {
        SourceKind::Postgres { .. } => Arc::new(PostgresSource::connect(cfg).await?),
        SourceKind::Bigquery { .. } => Arc::new(BigQuerySource::connect(cfg).await?),
    })
}

pub struct Entry { pub cfg: SourceConfig, pub source: Result<Arc<dyn DataSource>, String> }
static REGISTRY: OnceLock<Vec<Entry>>;
pub async fn init(cfgs: &[SourceConfig])   // build every enabled source; log "ready"/"unavailable: …"
pub fn entries() -> &'static [Entry];
pub fn get(id: &str) -> Result<&'static Entry, String>;          // "unknown data source …"
impl Entry {
    pub fn source(&self) -> Result<&Arc<dyn DataSource>, String>; // "data source … is unavailable: …"
    pub async fn tables(&self) -> Result<Vec<String>, String>;    // cfg.tables or list_tables()
    pub async fn info(&self) -> SourceInfo;                       // error filled from tables()
}
pub async fn infos() -> Vec<SourceInfo>;
```

Errors are `String`s with a prefix (`PostgreSQL: …`, `BigQuery: …`) — they are shown to users.

### 9.2 Filter compiler (`src/server/query.rs`)

`Params(Vec<String>)`; `bind(d, v)` pushes `v` and returns `d.param(len)`.
`like_escape(s)` escapes `\`, `%`, `_` with a backslash. `glob_to_like(s)` = `like_escape(s)` then
`*`→`%`, `?`→`_`. Let `id = d.quote_ident(col)`, `txt = d.text(col)`, `cs = case_sensitive`.

| Node / Op | SQL produced |
|---|---|
| Group | children compiled; `"TRUE"` results dropped; joined with ` AND ` / ` OR ` in `( … )`; no children → `TRUE`; `negate` → `NOT COALESCE(<body>, FALSE)` |
| unknown column | error `unknown column "<name>"` |
| IsNull / NotNull | `<id> IS NULL` / `<id> IS NOT NULL` |
| IsEmpty | `(<id> IS NULL OR TRIM(<txt>) IN ('', '{}', '[]'))` |
| (rule with no non-empty values) | `TRUE` (ignored) |
| Contains | `d.like(txt, bind("%"+esc(v)+"%"), cs)` |
| StartsWith / EndsWith | `d.like(txt, bind(esc(v)+"%"), cs)` / `d.like(txt, bind("%"+esc(v)), cs)` |
| Wildcard | `d.like(txt, bind(glob_to_like(v)), cs)` |
| Equals / NotEquals | `d.equals(txt, bind(v), cs)` |
| Regex / NotRegex | `d.regex(txt, bind(v), cs)` |
| Gt/Gte/Lt/Lte | `<id> <op> CAST(<bind(v)> AS <d.compare_type(col)>)`; `None` → error "can't be compared" |
| multiple values | each value compiled, joined `( a OR b )` |
| NotEquals / NotRegex | `NOT COALESCE(( … ), FALSE)` (NULL counts as "not equal") |

`prepare(req)`: registry lookup → `source()` → table must be in `entry.tables()` → `columns()` →
`build_where` → quick search (non-empty after trim) adds
`AND (like(text(c1), P, false) OR …)` with a single bound `%esc(quick)%` → `FROM <table_ref> WHERE …`
→ sort column = requested (if it exists) else first column; `ORDER BY <id or text(col) if !sortable> ASC|DESC NULLS LAST`.
Select list: `text(c_i) AS __c<i>` (the alias prefix prevents ORDER BY from binding to the text alias).

`search(req)`: `page_size` clamped 1..=200, `page ≥ 1`; runs **concurrently** (`tokio::try_join!`)
`SELECT <text_expr("COUNT(*)")> <from>` and `SELECT <list> <from><order> LIMIT n OFFSET m`;
`sql` field = `SELECT * <from><order><limit>\n-- params: $1::text = "…", …`.

`export(req)`: `SELECT <list> <from><order> LIMIT max_export_rows+1`; `truncated = rows > max`.

### 9.3 PostgreSQL adapter (`src/server/postgres.rs`)

- `pool(url, statement_timeout_ms, size)`: parse `tokio_postgres::Config`, set
  `options("-c statement_timeout=<ms>")` when > 0, `Manager::from_config(…, NoTls, RecyclingMethod::Fast)`,
  `Pool::builder(mgr).max_size(size)`. (Plain TCP; add `tokio-postgres-rustls` for TLS-only servers.)
- `db_err`: prefer `e.as_db_error().message()` → `"PostgreSQL: <message>"`.
- `connect`: build pool, `pool.get()` once to fail fast.
- `list_tables`: `SELECT table_name::text FROM information_schema.tables WHERE table_schema = $1 AND table_type IN ('BASE TABLE','VIEW') ORDER BY 1`.
- `columns`: `SELECT a.attname::text, format_type(a.atttypid, a.atttypmod) FROM pg_attribute a WHERE a.attrelid = (quote_ident($1::text) || '.' || quote_ident($2::text))::regclass AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum`.
- `query`: bind `&String` params; every column read as `Option<String>`.

`PgDialect`: `quote_ident` = `"` + name with `"`→`""` + `"` · `param(n)` = `$n::text` (explicit cast
so Postgres never has to infer a param type) · `text` = `"<col>"::text` · `text_expr(e)` = `(e)::text` ·
`like` = `ILIKE` / `LIKE` (cs) · `regex` = `~*` / `~` (cs) · `compare_type` = `col.ty` (from
`format_type`, catalog-generated) · `sortable` = false for `point json xml line lseg box path polygon circle`.

### 9.4 BigQuery adapter (`src/server/bigquery.rs`)

Auth: `BqAuth::Adc` → `gcp_auth::provider().await` (env `GOOGLE_APPLICATION_CREDENTIALS`, gcloud ADC
file, or GCE/GKE/Cloud Run metadata server); token scope `https://www.googleapis.com/auth/bigquery`,
sent as bearer. `BqAuth::None` → no header (emulator). `connect` calls `list_tables()` to fail fast.

URLs: `{endpoint}/bigquery/v2/projects/{project}/…` built with `Url::path_segments_mut()` (each
segment encoded).

| Operation | REST call |
|---|---|
| list tables | `GET …/datasets/{dataset}/tables?maxResults=1000[&pageToken=]` → `tables[].tableReference.tableId`, follow `nextPageToken`; cached (`tokio::sync::OnceCell`) |
| columns | `GET …/datasets/{dataset}/tables/{table}` → `schema.fields[]`; `ty = "ARRAY<T>"` if `mode == "REPEATED"` else `type`; cached per table (`Mutex<HashMap>`) |
| query | `POST …/queries` body `{query, useLegacySql:false, parameterMode:"NAMED", queryParameters:[{name:"p1", parameterType:{type:"STRING"}, parameterValue:{value}}…], timeoutMs, jobTimeoutMs, location?}` |
| poll / page | while `!jobComplete` or `pageToken` present: `GET …/queries/{jobReference.jobId}?timeoutMs=&location=[&pageToken=]` (pageToken only once complete) |
| rows | `rows[].f[].v` → `Some(string)` / `None` for JSON null |
| errors | non-2xx → `"BigQuery: " + body.error.message` |

`BqDialect`: `quote_ident` = backticks (escape `\` and `` ` ``); `table_ref` = quote of
`project.dataset.table` · `param(n)` = `@pn` · `text`: `STRING`→ident, `BYTES`→`TO_BASE64`,
`GEOGRAPHY`→`ST_ASTEXT`, `ARRAY<…>/RECORD/STRUCT/JSON/RANGE`→`TO_JSON_STRING`, else
`CAST(x AS STRING)` · `text_expr(e)` = `CAST(e AS STRING)` · `like` = `x LIKE p` (cs) else
`LOWER(x) LIKE LOWER(p)` · `regex` = `REGEXP_CONTAINS(x, p)` (cs) else
`REGEXP_CONTAINS(x, CONCAT('(?i)', p))` (RE2 syntax) · `compare_type`: INTEGER/INT64→INT64,
FLOAT/FLOAT64→FLOAT64, BOOLEAN/BOOL→BOOL, NUMERIC/BIGNUMERIC/STRING/DATE/DATETIME/TIME/TIMESTAMP→same,
else None · `sortable` = false for ARRAY/RECORD/STRUCT/JSON/GEOGRAPHY/RANGE.

### 9.5 Saved searches (`src/server/store.rs`)

Created automatically at startup (`CREATE … IF NOT EXISTS`) in the `[storage]` database:

```sql
CREATE TABLE IF NOT EXISTS saved_queries (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name        text NOT NULL,
    source      text NOT NULL,
    table_name  text NOT NULL,
    request     jsonb NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS saved_queries_created_at ON saved_queries (created_at DESC);
```

- `save(name, req)`: name trimmed, max 120 chars, empty → "Untitled search"; insert with
  `$4::text::jsonb`; `RETURNING id::text, name, to_char(created_at,'YYYY-MM-DD HH24:MI'), request::text`.
- `load(id)`: reject anything that is not a 36-char canonical UUID **before** querying
  (→ "saved query not found"); `WHERE id = $1::text::uuid`.
- `recent(limit)`: newest first.
- If storage can't be opened, the app still runs; save/load return "saved queries are disabled …".

### 9.6 Export (`src/server/export.rs`) — `GET /export/{format}?q=<SearchRequest JSON>`

- `format` ∈ `json | xlsx` else 404; bad JSON / query error → 400 with the message.
- Headers: `Content-Type` (`application/json` or
  `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`), `Content-Disposition:
  attachment; filename="<source>-<table>.<ext>"` (table sanitized to `[A-Za-z0-9-_]`),
  `x-export-truncated: true|false`.
- Numeric detection by base type (lowercased, before `(`): `smallint integer bigint int int2 int4
  int8 numeric decimal real "double precision" float float4 float8 int64 float64 bignumeric`.
- JSON: pretty array of objects (column order preserved); numeric columns → integer if it parses
  as i64, else finite f64, else string (`"NaN"` stays a string); `boolean/bool` columns → true/false; NULL → null.
- XLSX (`rust_xlsxwriter`): sheet "Data" — bold header row, column widths from header and first 200
  rows (max 60), numbers via `write_number` for numeric columns, strings truncated to 32 767 chars,
  `set_freeze_panes(1,0)`, autofilter over the data; sheet "Search" — Source, Table, Rows exported,
  Truncated, Quick search, Filter (JSON).

### 9.7 Server functions (`src/app.rs`, `#[server]`, mounted under `/api/…`)

| fn | Input codec | Returns | Notes |
|---|---|---|---|
| `list_sources()` | default | `Vec<SourceInfo>` | starts with `tokio::task::yield_now().await` (see §11 gotcha 6) |
| `get_columns(source, table)` | default | `Vec<Column>` | table must be exposed |
| `run_search(req)` | `server_fn::codec::Json` | `SearchPage` | nested enums need the JSON codec |
| `save_query(name, req)` | Json | `SavedQuery` | validates by running the search with page_size 1 |
| `load_query(id)` | default | `SavedQuery` | |
| `recent_queries()` | default | `Vec<SavedQuery>` | 20 newest |

Errors: `ServerFnError::new(msg)`; the UI shows `ServerFnError::ServerError(m)` → `m`.

### 9.8 `src/main.rs`

```rust
webui::server::init().await;                       // config → registry (factory) → storage
let conf = get_configuration(None).unwrap();       // from [package.metadata.leptos] / LEPTOS_* env
let routes = generate_route_list(App);
let app = Router::new()
    .route("/export/{format}", axum::routing::get(webui::server::export::handler))
    .leptos_routes(&leptos_options, routes, { let o = leptos_options.clone(); move || shell(o.clone()) })
    .fallback(leptos_axum::file_and_error_handler(shell))
    .with_state(leptos_options);
```

---

## 10. UI (`src/app.rs`, `style/main.css`)

### 10.1 Component tree

```
shell(options)               <html>, <AutoReload/>, <HydrationScripts/>, <MetaTags/>, <App/>
App                          <Stylesheet href="/pkg/webui.css"/>, <Title/>, <Router>
 ├─ Route ""      Home   ssr=SsrMode::Async   <Await future=list_sources()> → home_page(...)
 └─ Route "/s/:id" Shared ssr=SsrMode::Async  <Await future=(load_query, list_sources)>
     Explorer(init: SearchRequest, sources: Vec<SourceInfo>, saved: Option<SavedQuery>)
      ├─ header: brand, kind badge, <select> with <optgroup> per source (value "source/table";
      │          unavailable source → disabled "⚠ unavailable: <error>")
      ├─ banner (saved only): name, created, id
      ├─ toolbar: Save & share, ⬇ Excel, ⬇ JSON, <details> Saved searches
      ├─ save panel (toggle): name input + Save; result: share URL (readonly, select on focus) + Copy
      ├─ search card: quick search, Show SQL toggle, Reset, GroupView(root, depth 0)
      │    GroupView(group, depth, on_remove?) — recursive; returns AnyView
      │      └─ For children → .item (drop target) + ⠿ handle (draggable) + RuleView | GroupView
      │    RuleView(rule, on_remove) — column select, op select, chips + input, Aa, ✕
      └─ results card: <Transition set_pending> → ResultView(page) | error box
           ResultView: stats pills, SQL <pre>, table (sticky header, sort on click), pager
```

### 10.2 State (inside `Explorer`)

- Signals: `source`, `table`, `quick`, `sort`, `page`, `page_size`, `show_sql`, `(pending, set_pending)`.
- Filter tree as **nested signals** (so typing never re-renders the tree):
  `GroupState { id, join: RwSignal<Join>, negate: RwSignal<bool>, children: RwSignal<Vec<NodeState>> }`,
  `RuleState { id, column, op, values: RwSignal<Vec<String>>, case_sensitive }`,
  `enum NodeState { Rule(RuleState), Group(GroupState) }` (all `Copy`), ids from a global `AtomicU32`.
  `NodeState::from_filter(&Filter)` / `to_filter()` convert both ways (round-trip tested).
- `filter = Memo(root.to_filter())`; `request = Memo(SearchRequest{…})`;
  `results = Resource::new(move || request.get(), run_search)`.
- `Effect::watch((filter, quick, source, table, page_size), |_| page.set(1), false)` — any change
  that alters the result set returns to page 1.
- Contexts: `Columns = Signal<Vec<Column>>` (from `LocalResource(get_columns(source, table))`),
  `Owner` (Explorer's), `Dnd { dragging: RwSignal<Option<Drag>>, hover: RwSignal<Option<(group_id, Option<before_id>)>> }`.
- New rules/groups are created inside `owner.with(|| …)` — Explorer's owner — because they are
  created in event handlers and can later move between groups (see gotcha 4).
- URL sync: `Effect` → if `request != initial` → `history.replaceState(null, "", "/?q=…")`
  (not router navigation — that would remount the page).
- Export links: `href=/export/{fmt}?q=<request with page 1>`.
- Save: `spawn_local(save_query(name, request))` → show `location.origin + /s/<id>`,
  `replace_url("/s/<id>")`, `recent.refetch()`. Copy uses an inline-JS `navigator.clipboard.writeText`
  binding (`#[wasm_bindgen(inline_js = …)]`, hydrate-only; no-op on server).

### 10.3 Drag & drop rules

- `can_move(drag, to)`: rules always; a group never into itself or any descendant (`GroupState::contains`).
- `move_node(drag, to, before)`: no-op if not allowed or `before == dragged id`; remove from
  `drag.from`, insert into `to` before `before` (or append).
- Item `dragover` → `dnd.over(ev, group, Some(item_id))`; children container → `(group, None)`.
  `over` calls `prevent_default` + `stop_propagation` only when the move is allowed, sets `hover`
  only when it changed. `drop` performs the move then clears state; `dragend` clears state.
- `dragstart` sets `dataTransfer.effectAllowed = "move"` and `setData("text/plain", …)` (Firefox).
- CSS: `.item.drop-before` (cyan top line), `.children.drop-end` (dashed outline), `.item.dragging` (faded).

### 10.4 Cells

`NULL` → `.null` badge · blank → `.blank "empty"` · `boolean` column → `.bool` ✓ true / ✗ false ·
broken-looking (`#error*`, `err_*`, `nan`, `infinity`, `-infinity`, `-1`, contains `"error"`) →
`.bad ⚠ …` · otherwise `.val` with quick-search matches wrapped in `<mark>` (ASCII case folding).

### 10.5 Styling

Design tokens on `:root` (dark default; light overrides in `@media (prefers-color-scheme: light)`):
`--bg --bg2 --card --line --text --muted --accent(#7c5cff) --accent2(#22d3ee) --ok --warn --bad
--input --shadow --radius`. Glass cards (`backdrop-filter: blur`), radial gradient background,
nested groups coloured by depth (`--depth` → `hsl(calc(255 + depth*45) 85% 66%)` left border),
chips with gradient, sticky table header, zebra rows, animated progress bar while pending,
kind badges (`.kind[data-kind=postgres]` blue, `bigquery` amber). Mobile: rule grid collapses
under 760px.

---

## 11. Leptos gotchas (each one cost real debugging time — do not regress)

1. **`>` inside an attribute ends the tag in `view!`.** `disabled=p.page >= pages` silently breaks
   the element. Always brace expressions: `disabled={p.page >= pages}`, `class:nested={depth > 0}`.
2. **Never read a `LocalResource` inside `<Suspense>`.** The server renders the fallback, the
   client renders something else → "Unrecoverable hydration error". Read it in a plain
   `move || match res.get() { None => …, Some(r) => … }` (None on both sides during hydration).
3. **Never wrap a stateful page in `move || Suspend::new(async { res.await; … })`.** The closure
   re-runs when the resource resolves, so the page is built twice; the discarded copy's resources
   later touch disposed signals → server panics ("already been disposed") and the HTML comes out
   empty. Use `<Await future=… let:data>` whose children are `FnOnce` (rendered once).
4. **Signals created in event handlers** must be owned by a long-lived owner. We
   `provide_context(Owner::current())` in `Explorer` and create rules/groups with `owner.with(|| …)`,
   otherwise moving a node out of a group whose view is later removed disposes its signals.
5. **Don't use router `navigate` to keep the URL in sync** — it re-runs the route. Use
   `window().history()?.replace_state_with_url(…)` (web-sys feature `History`).
6. **`<Await>` + a future that is ready on the first poll renders nothing during SSR**
   (leptos 0.8.22). `list_sources()` does no I/O when tables are configured, so it starts with
   `tokio::task::yield_now().await`. Re-test without it when upgrading Leptos.
7. **Use `SsrMode::Async` on both routes** so the response contains the finished table (pure SSR),
   instead of streamed fragments.
8. **Server-function inputs with nested enums** need `#[server(input = server_fn::codec::Json)]`
   (the default URL-encoded codec can't express `Filter`).
9. **Postgres params must have explicit types**: we bind every value as text and write `$n::text`
   (e.g. `lower($1)` alone is ambiguous; `CAST($1 AS integer)` would make Postgres expect an int param).
10. **ORDER BY must not hit the text alias**: select list aliases are `__c0…`, ORDER BY uses the real
    quoted column (or its text form only when the type is unsortable).
11. **Unit tests that create signals** must run inside `Owner::new().with(|| …)`.
12. **Env expansion must skip comment lines** — documentation in the TOML mentions `${NAME}`.
13. **Release builds need `#![recursion_limit = "256"]`** at the top of `src/lib.rs`; without it the
    optimized WASM build fails with "queries overflow the depth limit" (dev builds don't show it,
    so always run `cargo leptos build --release` before shipping — Docker runs exactly that).
14. **Animations + screenshots/tests**: elements fade in (`@keyframes pop`); `prefers-reduced-motion`
    disables all animation (accessibility) — use it for deterministic screenshots.

---

## 12. Setup on a new machine (Mode A)

### 12.1 Prerequisites

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add wasm32-unknown-unknown
cargo install cargo-leptos --version 0.3.11 --locked   # takes a few minutes

# PostgreSQL 13+ client+server
brew install postgresql@14                   # macOS
sudo apt-get install postgresql postgresql-contrib   # Debian/Ubuntu (then use pg_ctlcluster or a project-local initdb as below)
```

### 12.2 Project-local database (does not touch any system DB)

```bash
cd <repo>
initdb -D ./pgdata -U postgres --auth=trust          # on Linux: /usr/lib/postgresql/<v>/bin/initdb
pg_ctl -D ./pgdata -o "-p 5433 -k /tmp" -l pgdata/log.txt start
createdb -h localhost -p 5433 -U postgres webui
psql -h localhost -p 5433 -U postgres -d webui -f db/seed.sql
```

Using an existing server instead: set `DATABASE_URL` and `STORAGE_DATABASE_URL`
(e.g. `host=db.internal port=5432 user=app password=… dbname=analytics`) — `config/app.toml`
picks them up — and run `db/seed.sql` there if you want the mock tables.

### 12.3 Run

```bash
cargo leptos watch          # dev: http://127.0.0.1:3000 (auto rebuild + browser reload)
cargo test --features ssr --lib
```

Expected startup log:
```
source "local_pg" (postgres) ready
source "bq_demo" (bigquery) unavailable: no GCP credentials: …   # until BigQuery is configured
listening on http://127.0.0.1:3000
```

### 12.4 Production build

```bash
cargo leptos build --release
# outputs: target/release/webui  and  target/site/ (pkg/webui.js, .wasm, .css)
LEPTOS_OUTPUT_NAME=webui LEPTOS_SITE_ROOT=site LEPTOS_SITE_PKG_DIR=pkg \
LEPTOS_SITE_ADDR=0.0.0.0:3000 LEPTOS_RELOAD_PORT=3001 \
APP_CONFIG=/etc/webui/app.toml DATABASE_URL=… STORAGE_DATABASE_URL=… \
./webui        # run from a directory containing the copied `site/` folder
```

Container: see §12.6 — the repo's `Dockerfile` does exactly this.

### 12.5 BigQuery

1. Create/choose a GCP project and dataset; note its location.
2. Credentials (any one): `gcloud auth application-default login` (developer machine);
   a service account key with `export GOOGLE_APPLICATION_CREDENTIALS=/path/key.json`;
   or run on GCP (Cloud Run/GKE/GCE) with an attached service account.
3. Roles for that identity: **BigQuery Data Viewer** on the dataset and **BigQuery Job User** on
   the project that runs queries.
4. In `config/app.toml` set `project`, `dataset`, `location`, optionally `tables = [...]`;
   `enabled = true`; restart. The picker shows the dataset's tables with an amber BIGQUERY badge.
5. Optional sample data (BigQuery console):
   ```sql
   CREATE TABLE demo.customers AS
   SELECT * FROM `bigquery-public-data.thelook_ecommerce.users` LIMIT 1000;
   ```
6. Local emulator (no GCP needed): `docker run -p 9050:9050 ghcr.io/goccy/bigquery-emulator:latest
   --project=test --dataset=demo` and set `endpoint = "http://localhost:9050"`, `auth = "none"`,
   `project = "test"`. Emulator SQL coverage is partial — verify against real BigQuery before relying on it.

Cost note: every page view runs a COUNT and a page query; BigQuery bills scanned bytes. Prefer
partitioned/clustered tables or views that limit columns for large datasets.

### 12.6 Docker

```bash
docker compose up --build                 # app + postgres:16 seeded with db/seed.sql → http://localhost:3000
docker build -t table-explorer .          # image only
docker run -p 3000:3000 -e DATABASE_URL=… -e STORAGE_DATABASE_URL=… table-explorer
```

Image layout: `/app/webui`, `/app/site/pkg/*`, `/app/config/app.toml`; env `LEPTOS_OUTPUT_NAME=webui`,
`LEPTOS_SITE_ROOT=site`, `LEPTOS_SITE_PKG_DIR=pkg`, `LEPTOS_SITE_ADDR=0.0.0.0:3000`,
`APP_CONFIG=/app/config/app.toml`; non-root uid 10001. Mount a config over `/app/config/app.toml`
to change sources; mount a GCP key and set `GOOGLE_APPLICATION_CREDENTIALS` for BigQuery.
Full guide: [docs/DOCKER_DEPLOYMENT.md](docs/DOCKER_DEPLOYMENT.md).

---

## 13. Extending

### Add tables to an existing source
Edit that source's `tables = [...]` (or leave it empty to expose everything) → restart. Nothing else.

### Add another PostgreSQL database or BigQuery dataset
Append a `[[source]]` block with a new `id` → restart. It appears as a new group in the picker.

### Add a new engine (e.g. MySQL, Snowflake, DuckDB, ClickHouse)
1. `config.rs`: add a `SourceKind::<Name> { …settings }` variant and its `name()` string.
2. New file `src/server/<name>.rs`: `struct <Name>Source` + `impl DataSource` (5 methods) +
   `struct <Name>Dialect` + `impl Dialect` (8 methods). Every selected column must come back as text.
3. `source.rs` → `create`: one match arm.
4. `server/mod.rs`: `pub mod <name>;`; `Cargo.toml`: optional client crate added to `ssr`.
5. UI: add the label in `kind_label()` and a `.kind[data-kind="<name>"]` colour in CSS.
6. Tests: add a `build_where` test for the new dialect next to the existing ones in `query.rs`.

### Add an operator
`model.rs` (`Op`, `ALL`, `label`, `needs_value`) → `query.rs::build_where` (one arm, uses only
`Dialect` methods; add a method to `Dialect` if engines differ) → `placeholder()` in `app.rs` →
tests for both dialects.

---

## 14. Verification checklist (run all of these)

Unit tests: `cargo test --features ssr --lib` → **13 passed** (config ×3, query/dialects ×3,
bigquery ×2, store ×1, export ×2, UI state ×2).

Against the seed data (psql port 5433), expected counts:

| Search | Rows |
|---|---|
| none (customers) | 1000 |
| country contains `india` | 120 |
| country equals `USA`,`india` AND age ≥ 30 AND age ≤ 50 | 80 |
| email regex `^(alice\|bob)\d+@` AND (balance > 5000 OR notes is NULL) | 112 |
| active equals true AND ((email regex … AND balance > 5000) OR (tags contains vip AND country ≠ USA) OR notes is NULL) | 163 |
| (notes pattern `Order #10*DHL` OR notes starts with `#ERROR`) AND NOT (profile contains enterprise OR (signup_at ≥ 2022-01-01 AND signup_at < 2023-01-01)) | 23 |
| country equals USA, india AND (tags contains vip OR notes pattern `Order #10?? *DHL`) | 44 |
| sensor_readings: status equals `critical`, `ERR_TIMEOUT` | 229 |

HTTP checks:
```bash
curl -s http://127.0.0.1:3000/ | grep -o '[0-9]* matching rows'                 # 1000 matching rows (SSR)
curl -s http://127.0.0.1:3000/s/00000000-0000-0000-0000-000000000000 | grep -o 'saved query not found'
curl -s -o /dev/null -w '%{http_code}\n' 'http://127.0.0.1:3000/export/csv?q=%7B%7D'   # 404
curl -s -w '%{http_code}\n' 'http://127.0.0.1:3000/export/json?q=%7Bbad'               # 400
```

Browser checks (no console errors, especially no "hydration error"):
1. Picker shows "Local PostgreSQL" (customers, sensor_readings) and an unavailable BigQuery group.
2. Build the 163-row search above with nested groups; Show SQL shows the expected WHERE.
3. Invalid regex `(unclosed[` → red error box "PostgreSQL: invalid regular expression…".
4. Sort a header (asc/desc), Next/Last page, page size 100 → counts/ranges update; ids sort numerically.
5. Drag a rule from a nested group to the top level → count changes accordingly; dragging a group onto its own child does nothing.
6. ⬇ Excel / ⬇ JSON → files contain all matching rows (e.g. 80 for the 80-row search), not just one page.
7. Save & share → `/s/<uuid>` link; open it in a private window → same table, banner, filters; the server HTML already contains the rows (`curl … | grep 'matching rows'`).
8. Reload a `/?q=` URL → identical state. Edit something on a `/s/` page → URL switches to `/?q=`.

---

## 15. Security notes

- Values are always bound parameters. Identifiers come only from the catalog/API (`columns()`),
  are matched exactly against the request, and are quoted by the dialect. Table names must be in
  the source's exposed list. Comparison casts use catalog type names, never user text.
- `saved_queries` ids are validated as UUIDs before any query.
- Export is capped per source; page size capped at 200; Postgres `statement_timeout`, BigQuery
  `jobTimeoutMs` bound runaway regexes/scans.
- There is **no authentication**: anyone who can reach the app can query every exposed table and
  read every saved search (UUIDs are unguessable but not secret). Put it behind SSO/an auth proxy,
  and expose only tables meant for its users. Use least-privilege DB users (SELECT only on exposed
  tables; INSERT/SELECT on `saved_queries` for storage).
- Secrets belong in env vars referenced as `${…}` from the config, not in the file.

---

## 16. Build order for a from-scratch rebuild (Mode B)

| Phase | Do | Check |
|---|---|---|
| 1 | `cargo new webui`, write Cargo.toml (§6), `src/lib.rs`, `src/main.rs` with a hello `App`, `style/main.css` stub | `cargo leptos watch` serves a page |
| 2 | Postgres local + `db/seed.sql` (§12.2; schema in §17) | `select count(*)` = 1000 for both tables |
| 3 | `model.rs` (§8) | compiles for ssr and wasm |
| 4 | `config.rs` + `config/app.toml` (§7) with tests | `cargo test` config tests pass |
| 5 | `source.rs` traits/factory/registry (§9.1), `postgres.rs` (§9.3) | server starts, logs "ready" |
| 6 | `query.rs` (§9.2) with the dialect tests | tests pass; SQL strings match exactly |
| 7 | server fns + `Explorer` with results table, pager, sort (§10) | `/` shows 1000 rows SSR; paging works |
| 8 | Filter builder (GroupView/RuleView, nested signals) | counts in §14 match |
| 9 | URL state + `Home`/`Shared` with `<Await>` + `SsrMode::Async` (§10.2, §11) | reload of `/?q=` restores state |
| 10 | Drag & drop (§10.3) + tests | drag checks in §14 |
| 11 | `store.rs` + save/share UI (§9.5) | `/s/<uuid>` works in a private window |
| 12 | `export.rs` + toolbar links (§9.6) | exported row counts match |
| 13 | `bigquery.rs` (§9.4) + tests; enable a BigQuery source | parsing tests; real dataset or emulator shows tables |
| 14 | Styling polish (§10.5), README | visual check light/dark and ≤ 760px width |
| 15 | Dockerfile + compose (§12.6) | `cargo leptos build --release` succeeds; `docker compose up --build` serves 1000 rows |

---

## 17. Seed data (`db/seed.sql`) summary

Two tables generated with `generate_series(1, 1000)` and modulo rules so the data is deterministic:

- `customers(id serial PK, uid uuid, name text, email text, age int, balance numeric(12,2),
  active boolean, country varchar(40), tags text[], profile jsonb, signup_at timestamptz, notes text)`
  — NULLs at different moduli per column, `'not-an-email'` emails, `age = -1`, `balance = 'NaN'`,
  empty-string countries, empty arrays, `{"error": "profile sync failed", "code": 500}` profiles,
  `'#ERROR! value could not be parsed'` notes, names including `Müller` and `O'Brien`.
- `sensor_readings(reading_id bigserial PK, device text NOT NULL, location point,
  temperature double precision, humidity real, status text, recorded_on date, raw_payload bytea,
  meta json)` — `Infinity`/`NaN` temperatures, `ERR_TIMEOUT` statuses, NULLs.

The exact generator expressions are in `db/seed.sql`; reproduce that file verbatim so the
expected counts in §14 hold.

---

## 18. Known limitations (deliberate shortcuts)

- Config changes need a restart; BigQuery table/column metadata is cached until restart.
- Postgres connections are plain TCP (`NoTls`).
- BigQuery: only top-level columns are filterable (nested STRUCT fields are searched via their JSON text); regex is RE2 (no backreferences/lookaround).
- JSON export converts decimals through f64 (>15 significant digits lose precision).
- Quick-search highlighting uses ASCII case folding (non-ASCII matches still filter, just aren't highlighted).
- No auth, no per-user saved-search ownership, no deletion of saved searches from the UI.
- A source that fails at startup stays unavailable until restart.
