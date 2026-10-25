//! The data-source abstraction: a `Dialect` (strategy) per SQL engine, a `DataSource` per
//! backend, a factory that builds one from config, and the registry the app queries.
//!
//! Adding a backend = implement `Dialect` + `DataSource`, add a `SourceKind` variant and one
//! arm in `create`. Adding tables = edit `config/app.toml`.
use super::config::{SourceConfig, SourceKind};
use super::{bigquery::BigQuerySource, postgres::PostgresSource};
use crate::model::{Column, SourceInfo};
use async_trait::async_trait;
use std::sync::{Arc, OnceLock};

/// Result rows; every cell is the engine's text rendering of the value, `None` = NULL.
pub type Rows = Vec<Vec<Option<String>>>;

/// Everything the shared query compiler (`query.rs`) needs to know about an SQL engine.
pub trait Dialect: Send + Sync {
    fn quote_ident(&self, name: &str) -> String;
    /// Placeholder for the n-th (1-based) parameter; all parameters are sent as strings.
    fn param(&self, n: usize) -> String;
    /// The column rendered as text (what the UI shows and text operators match against).
    fn text(&self, col: &Column) -> String;
    /// An arbitrary expression rendered as text, e.g. `COUNT(*)`.
    fn text_expr(&self, expr: &str) -> String;
    fn like(&self, lhs: &str, pattern: &str, case_sensitive: bool) -> String;
    fn regex(&self, lhs: &str, pattern: &str, case_sensitive: bool) -> String;
    /// Type name to CAST a string parameter to for `> < >= <=`; `None` = not comparable.
    fn compare_type(&self, col: &Column) -> Option<String>;
    /// False for types the engine can't ORDER BY (they are sorted by their text form).
    fn sortable(&self, col: &Column) -> bool;

    fn equals(&self, lhs: &str, rhs: &str, case_sensitive: bool) -> String {
        if case_sensitive {
            format!("{lhs} = {rhs}")
        } else {
            format!("LOWER({lhs}) = LOWER({rhs})")
        }
    }
}

/// One configured backend. Adapters only provide these primitives; filtering, paging,
/// sorting and export are shared in `query.rs` (template method).
#[async_trait]
pub trait DataSource: Send + Sync {
    fn dialect(&self) -> &dyn Dialect;
    /// Fully-qualified, quoted reference to `table`.
    fn table_ref(&self, table: &str) -> String;
    async fn list_tables(&self) -> Result<Vec<String>, String>;
    async fn columns(&self, table: &str) -> Result<Vec<Column>, String>;
    /// Runs `sql` (whose select list is all text) with string parameters bound in order.
    async fn query(&self, sql: &str, params: &[String]) -> Result<Rows, String>;
}

/// Factory: the only place that maps a config `kind` to an adapter.
pub async fn create(cfg: &SourceConfig) -> Result<Arc<dyn DataSource>, String> {
    Ok(match &cfg.kind {
        SourceKind::Postgres { .. } => Arc::new(PostgresSource::connect(cfg).await?),
        SourceKind::Bigquery { .. } => Arc::new(BigQuerySource::connect(cfg).await?),
    })
}

pub struct Entry {
    pub cfg: SourceConfig,
    /// A source that failed to start stays listed (with its error) instead of crashing the app.
    pub source: Result<Arc<dyn DataSource>, String>,
}

static REGISTRY: OnceLock<Vec<Entry>> = OnceLock::new();

pub async fn init(cfgs: &[SourceConfig]) {
    let mut entries = Vec::new();
    for cfg in cfgs.iter().filter(|c| c.enabled) {
        let source = create(cfg).await;
        match &source {
            Ok(_) => println!("source {:?} ({}) ready", cfg.id, cfg.kind.name()),
            Err(e) => eprintln!("source {:?} ({}) unavailable: {e}", cfg.id, cfg.kind.name()),
        }
        entries.push(Entry { cfg: cfg.clone(), source });
    }
    let _ = REGISTRY.set(entries);
}

pub fn entries() -> &'static [Entry] {
    REGISTRY.get().map(Vec::as_slice).unwrap_or_default()
}

pub fn get(id: &str) -> Result<&'static Entry, String> {
    entries().iter().find(|e| e.cfg.id == id).ok_or_else(|| format!("unknown data source {id:?}"))
}

impl Entry {
    pub fn source(&self) -> Result<&Arc<dyn DataSource>, String> {
        self.source.as_ref().map_err(|e| format!("data source {:?} is unavailable: {e}", self.cfg.id))
    }

    /// Tables this source exposes: the configured list, or everything the backend reports.
    pub async fn tables(&self) -> Result<Vec<String>, String> {
        if !self.cfg.tables.is_empty() {
            return Ok(self.cfg.tables.clone());
        }
        self.source()?.list_tables().await
    }

    pub async fn info(&self) -> SourceInfo {
        let tables = self.tables().await;
        SourceInfo {
            id: self.cfg.id.clone(),
            label: self.cfg.label.clone(),
            kind: self.cfg.kind.name().into(),
            error: tables.as_ref().err().cloned(),
            tables: tables.unwrap_or_default(),
        }
    }
}

pub async fn infos() -> Vec<SourceInfo> {
    let mut out = Vec::new();
    for e in entries() {
        out.push(e.info().await);
    }
    out
}
