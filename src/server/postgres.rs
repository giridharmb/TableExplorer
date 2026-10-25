//! PostgreSQL adapter (also used for the saved-query storage pool).
use super::config::{SourceConfig, SourceKind};
use super::source::{DataSource, Dialect, Rows};
use crate::model::Column;
use async_trait::async_trait;
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use tokio_postgres::{types::ToSql, NoTls};

// shortcut: plain TCP (NoTls); add tokio-postgres-rustls here for managed/cloud Postgres that requires TLS.
pub fn pool(url: &str, statement_timeout_ms: u64, size: usize) -> Result<Pool, String> {
    let mut pg: tokio_postgres::Config = url.parse().map_err(|e| format!("bad postgres url: {e}"))?;
    if statement_timeout_ms > 0 {
        // A user-supplied regex can be pathological; cap every query.
        pg.options(format!("-c statement_timeout={statement_timeout_ms}"));
    }
    let mgr = Manager::from_config(pg, NoTls, ManagerConfig { recycling_method: RecyclingMethod::Fast });
    Pool::builder(mgr).max_size(size).build().map_err(|e| e.to_string())
}

pub fn db_err(e: tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(d) => format!("PostgreSQL: {}", d.message()),
        None => format!("PostgreSQL: {e}"),
    }
}

pub struct PostgresSource {
    pool: Pool,
    schema: String,
}

impl PostgresSource {
    pub async fn connect(cfg: &SourceConfig) -> Result<Self, String> {
        let SourceKind::Postgres { url, schema, statement_timeout_ms, pool_size } = &cfg.kind else {
            unreachable!("factory passes postgres configs only")
        };
        let pool = pool(url, *statement_timeout_ms, *pool_size)?;
        let _ = pool.get().await.map_err(|e| format!("cannot connect: {e}"))?; // fail fast at startup
        Ok(PostgresSource { pool, schema: schema.clone() })
    }

    async fn client(&self) -> Result<deadpool_postgres::Object, String> {
        self.pool.get().await.map_err(|e| format!("PostgreSQL pool: {e}"))
    }
}

pub struct PgDialect;

impl Dialect for PgDialect {
    fn quote_ident(&self, name: &str) -> String {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
    fn param(&self, n: usize) -> String {
        format!("${n}::text")
    }
    fn text(&self, col: &Column) -> String {
        format!("{}::text", self.quote_ident(&col.name))
    }
    fn text_expr(&self, expr: &str) -> String {
        format!("({expr})::text")
    }
    fn like(&self, lhs: &str, pattern: &str, cs: bool) -> String {
        format!("{lhs} {} {pattern}", if cs { "LIKE" } else { "ILIKE" })
    }
    fn regex(&self, lhs: &str, pattern: &str, cs: bool) -> String {
        format!("{lhs} {} {pattern}", if cs { "~" } else { "~*" })
    }
    fn compare_type(&self, col: &Column) -> Option<String> {
        // `format_type` output from the catalog, e.g. "numeric(12,2)" — not user input.
        Some(col.ty.clone())
    }
    fn sortable(&self, col: &Column) -> bool {
        !matches!(
            col.ty.as_str(),
            "point" | "json" | "xml" | "line" | "lseg" | "box" | "path" | "polygon" | "circle"
        )
    }
}

#[async_trait]
impl DataSource for PostgresSource {
    fn dialect(&self) -> &dyn Dialect {
        &PgDialect
    }

    fn table_ref(&self, table: &str) -> String {
        format!("{}.{}", PgDialect.quote_ident(&self.schema), PgDialect.quote_ident(table))
    }

    async fn list_tables(&self) -> Result<Vec<String>, String> {
        let rows = self
            .client()
            .await?
            .query(
                "SELECT table_name::text FROM information_schema.tables
                 WHERE table_schema = $1 AND table_type IN ('BASE TABLE', 'VIEW') ORDER BY 1",
                &[&self.schema],
            )
            .await
            .map_err(db_err)?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    async fn columns(&self, table: &str) -> Result<Vec<Column>, String> {
        let rows = self
            .client()
            .await?
            .query(
                "SELECT a.attname::text, format_type(a.atttypid, a.atttypmod)
                 FROM pg_attribute a
                 WHERE a.attrelid = (quote_ident($1::text) || '.' || quote_ident($2::text))::regclass
                   AND a.attnum > 0 AND NOT a.attisdropped
                 ORDER BY a.attnum",
                &[&self.schema, &table],
            )
            .await
            .map_err(db_err)?;
        Ok(rows.iter().map(|r| Column { name: r.get(0), ty: r.get(1) }).collect())
    }

    async fn query(&self, sql: &str, params: &[String]) -> Result<Rows, String> {
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|s| s as &(dyn ToSql + Sync)).collect();
        let rows = self.client().await?.query(sql, &refs).await.map_err(db_err)?;
        Ok(rows.iter().map(|r| (0..r.len()).map(|i| r.get::<_, Option<String>>(i)).collect()).collect())
    }
}
