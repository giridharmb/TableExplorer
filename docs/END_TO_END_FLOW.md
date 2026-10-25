# End-to-end flow

How a request travels from the browser to PostgreSQL / BigQuery and back. Diagrams are Mermaid
(rendered by GitHub). File references are to `src/`.

## 1. Big picture

```mermaid
flowchart LR
    subgraph Browser
        UI["Leptos UI (WASM)<br/>Explorer · GroupView · RuleView · ResultView"]
    end
    subgraph Server["Rust server (axum + Leptos SSR)"]
        SSR["SSR routes<br/>/ and /s/:id"]
        FN["Server functions<br/>/api/run_search, save_query, …"]
        EXP["GET /export/{xlsx|json}"]
        Q["query.rs<br/>compile filter → SQL<br/>count + page / export"]
        REG["source.rs registry<br/>(built by factory from config)"]
        PG["PostgresSource + PgDialect"]
        BQ["BigQuerySource + BqDialect"]
        ST["store.rs<br/>saved_queries"]
    end
    CFG[("config/app.toml")]
    PGDB[("PostgreSQL")]
    BQDB[("Google BigQuery")]

    UI -- "first load (HTML)" --> SSR
    UI -- "JSON" --> FN
    UI -- "download link" --> EXP
    SSR --> Q
    FN --> Q
    FN --> ST
    EXP --> Q
    Q --> REG
    REG --> PG --> PGDB
    REG --> BQ -- "REST v2" --> BQDB
    ST --> PGDB
    CFG -. startup .-> REG
```

## 2. Startup

```mermaid
sequenceDiagram
    participant Main as main.rs
    participant Cfg as config.rs
    participant Reg as source.rs (registry)
    participant Fac as source::create (factory)
    participant Store as store.rs
    Main->>Cfg: AppConfig::load()  (APP_CONFIG or config/app.toml, ${ENV} expansion)
    Main->>Reg: init(sources)
    loop every enabled [[source]]
        Reg->>Fac: create(cfg)
        alt kind = postgres
            Fac-->>Reg: PostgresSource (pool + test connection)
        else kind = bigquery
            Fac-->>Reg: BigQuerySource (ADC token + list tables)
        end
        Note over Reg: a failure is kept as Err(reason)<br/>→ shown as "unavailable" in the UI
    end
    Main->>Store: init(storage.postgres_url)  CREATE TABLE IF NOT EXISTS saved_queries
    Main->>Main: axum Router: /export/{format} + Leptos routes + static /pkg
```

## 3. First page load (server-side rendering)

```mermaid
sequenceDiagram
    participant B as Browser
    participant S as Server (Leptos SSR, SsrMode::Async)
    participant Q as query::search
    participant D as DataSource
    B->>S: GET /?q=<SearchRequest JSON>   (or GET /)
    S->>S: Home: parse q (bad/missing → first available table)
    S->>S: <Await> list_sources()  → table picker data
    S->>Q: Explorer's results Resource → run_search(request)
    Q->>D: columns(table)            (introspection)
    Q->>Q: build_where(filter, columns, dialect) → SQL + params
    par count and page run concurrently
        Q->>D: SELECT COUNT(*) FROM … WHERE …
    and
        Q->>D: SELECT text(cols) FROM … WHERE … ORDER BY … LIMIT n OFFSET m
    end
    D-->>Q: rows (every cell as text or NULL)
    Q-->>S: SearchPage
    S-->>B: complete HTML table + serialized resource data + <script> webui.js/.wasm
    B->>B: hydrate: reuse serialized data (no second query), attach event handlers
```

## 4. Searching, sorting, paging (after hydration)

```mermaid
sequenceDiagram
    participant U as User
    participant W as Explorer (WASM signals)
    participant S as POST /api/run_search
    participant Q as query.rs
    U->>W: edit rule / add chip / drag rule / click header / Next ›
    W->>W: filter Memo → request Memo (source, table, filter, quick, sort, page, page_size)
    W->>W: page reset to 1 when filter/quick/table/page size change
    W->>W: history.replaceState("/?q=…")   (address bar = shareable link)
    W->>S: run_search(request)  (JSON)
    S->>Q: prepare → compile → count + page
    Q-->>S: SearchPage or error string
    S-->>W: JSON
    W-->>U: <Transition>: old table dimmed + progress bar, then new rows<br/>errors → red "Query failed" box with the database message
```

### How a filter becomes SQL

```mermaid
flowchart TD
    F["Filter tree (JSON)<br/>Group AND<br/>├ Rule country equals [USA, india]<br/>└ Group OR<br/>&nbsp;&nbsp;├ Rule tags contains [vip]<br/>&nbsp;&nbsp;└ Rule notes pattern [Order #10?? *DHL]"]
    C["build_where + Dialect"]
    P["PostgreSQL<br/>((LOWER(#quot;country#quot;::text) = LOWER($1::text) OR LOWER(#quot;country#quot;::text) = LOWER($2::text))<br/>AND ((#quot;tags#quot;::text ILIKE $3::text) OR (#quot;notes#quot;::text ILIKE $4::text)))"]
    B["BigQuery<br/>((LOWER(`country`) = LOWER(@p1) OR LOWER(`country`) = LOWER(@p2))<br/>AND ((LOWER(TO_JSON_STRING(`tags`)) LIKE LOWER(@p3)) OR (LOWER(`notes`) LIKE LOWER(@p4))))"]
    V["params: USA · india · %vip% · Order #10__ %DHL"]
    F --> C --> P
    C --> B
    C --> V
```

Values are always bound parameters; column names come from introspection and are quoted by the
dialect. Full operator table: [AI_AGENT_INSTRUCTIONS.md §9.2](../AI_AGENT_INSTRUCTIONS.md#92-filter-compiler-srcserverqueryrs).

## 5. Save & share

```mermaid
sequenceDiagram
    participant U as User A
    participant W as Explorer (WASM)
    participant S as Server
    participant DB as PostgreSQL (storage)
    participant V as User B
    U->>W: 🔗 Save & share → name → Save
    W->>S: save_query(name, request)
    S->>S: validate: run the search with page_size 1
    S->>DB: INSERT INTO saved_queries (name, source, table_name, request jsonb) RETURNING id
    DB-->>S: uuid
    S-->>W: SavedQuery
    W-->>U: http://host/s/<uuid>  (+ Copy), address bar replaced
    U-->>V: sends the link
    V->>S: GET /s/<uuid>
    S->>DB: SELECT … WHERE id = $1::uuid   (non-UUID → "not found" without querying)
    S->>S: Shared: <Await> load_query + list_sources → Explorer(init = saved.request)
    S-->>V: fully rendered table + 📌 banner, same filters
    Note over V: editing anything switches the address bar to /?q=… (the saved link stays unchanged)
```

## 6. Export

```mermaid
sequenceDiagram
    participant U as User
    participant S as GET /export/{xlsx|json}?q=…
    participant Q as query::export
    participant D as DataSource
    U->>S: click ⬇ Excel / ⬇ JSON (href built from the current request)
    S->>S: parse q (bad → 400) · format must be xlsx|json (else 404)
    S->>Q: export(request)
    Q->>D: SELECT … WHERE … ORDER BY … LIMIT max_export_rows + 1   (no paging)
    D-->>Q: rows
    Q-->>S: rows (truncated flag if > max)
    alt xlsx
        S->>S: rust_xlsxwriter: Data sheet (bold frozen header, autofilter, numbers typed) + Search sheet
    else json
        S->>S: [{column: value}] in column order, numbers/booleans typed
    end
    S-->>U: attachment <source>-<table>.<ext>, header x-export-truncated
```

## 7. Plugging in another source

```mermaid
flowchart LR
    A["Add [[source]] to config/app.toml"] --> B["restart"]
    B --> C["factory builds adapter<br/>(by kind)"]
    C --> D["registry entry"]
    D --> E["list_sources() → picker shows a new group with a badge"]
    N["New engine (e.g. MySQL)"] --> N1["impl DataSource + Dialect<br/>(src/server/mysql.rs)"]
    N1 --> N2["SourceKind::Mysql + one arm in source::create"]
    N2 --> A
```
