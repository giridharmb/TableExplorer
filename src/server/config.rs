//! `config/app.toml` (or `$APP_CONFIG`): storage DB + the list of data sources.
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub storage: StorageConfig,
    #[serde(default, rename = "source")]
    pub sources: Vec<SourceConfig>,
}

/// Where saved/shared queries are stored (always PostgreSQL).
#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub postgres_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceConfig {
    /// Stable id used in URLs and saved queries: lowercase letters, digits, `_`, `-`.
    pub id: String,
    pub label: String,
    /// Tables shown in the UI; empty = every table/view in the schema or dataset.
    #[serde(default)]
    pub tables: Vec<String>,
    #[serde(default = "default_max_export_rows")]
    pub max_export_rows: u64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(flatten)]
    pub kind: SourceKind,
}

/// Adapter-specific settings; `kind = "..."` picks the adapter in `source::create`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SourceKind {
    Postgres {
        url: String,
        #[serde(default = "default_schema")]
        schema: String,
        #[serde(default = "default_timeout_ms")]
        statement_timeout_ms: u64,
        #[serde(default = "default_pool_size")]
        pool_size: usize,
    },
    Bigquery {
        project: String,
        dataset: String,
        /// Dataset location, e.g. "US" or "europe-west2".
        location: Option<String>,
        /// Override for the BigQuery emulator, e.g. "http://localhost:9050".
        #[serde(default = "default_bq_endpoint")]
        endpoint: String,
        #[serde(default)]
        auth: BqAuth,
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
}

impl SourceKind {
    pub fn name(&self) -> &'static str {
        match self {
            SourceKind::Postgres { .. } => "postgres",
            SourceKind::Bigquery { .. } => "bigquery",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BqAuth {
    /// Application Default Credentials: GOOGLE_APPLICATION_CREDENTIALS, gcloud, or the metadata server.
    #[default]
    Adc,
    /// No auth header (BigQuery emulator).
    None,
}

fn default_max_export_rows() -> u64 {
    100_000
}
fn default_true() -> bool {
    true
}
fn default_schema() -> String {
    "public".into()
}
fn default_timeout_ms() -> u64 {
    30_000
}
fn default_pool_size() -> usize {
    8
}
fn default_bq_endpoint() -> String {
    "https://bigquery.googleapis.com".into()
}

impl AppConfig {
    pub fn load() -> Result<Self, String> {
        let path = std::env::var("APP_CONFIG").unwrap_or_else(|_| "config/app.toml".into());
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        Self::parse(&raw).map_err(|e| format!("{path}: {e}"))
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        let cfg: AppConfig = toml::from_str(&expand_env(raw)?).map_err(|e| e.to_string())?;
        let mut seen = std::collections::HashSet::new();
        for s in &cfg.sources {
            let ok = !s.id.is_empty()
                && s.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
            if !ok {
                return Err(format!("source id {:?} must use only a-z, 0-9, _ and -", s.id));
            }
            if !seen.insert(&s.id) {
                return Err(format!("duplicate source id {:?}", s.id));
            }
        }
        Ok(cfg)
    }
}

/// Replaces `${VAR}` / `${VAR:-default}` so secrets can stay in the environment.
/// Comment lines are left alone so documentation can mention the syntax.
fn expand_env(s: &str) -> Result<String, String> {
    let lines: Result<Vec<String>, String> = s
        .lines()
        .map(|l| if l.trim_start().starts_with('#') { Ok(l.to_string()) } else { expand_line(l) })
        .collect();
    Ok(lines?.join("\n"))
}

fn expand_line(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}').ok_or("unterminated ${ in config")? + start;
        let expr = &rest[start + 2..end];
        let (name, default) = match expr.split_once(":-") {
            Some((n, d)) => (n, Some(d)),
            None => (expr, None),
        };
        match (std::env::var(name), default) {
            (Ok(v), _) => out.push_str(&v),
            (Err(_), Some(d)) => out.push_str(d),
            (Err(_), None) => return Err(format!("environment variable {name} is not set")),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_source_kinds_with_env_defaults() {
        let cfg = AppConfig::parse(
            r#"
            [storage]
            postgres_url = "${WEBUI_TEST_UNSET_VAR:-host=localhost}"

            [[source]]
            id = "pg"
            label = "PG"
            kind = "postgres"
            url = "host=db"
            tables = ["a"]

            [[source]]
            id = "bq"
            label = "BQ"
            kind = "bigquery"
            project = "p"
            dataset = "d"
            auth = "none"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.storage.postgres_url, "host=localhost");
        assert!(matches!(&cfg.sources[0].kind, SourceKind::Postgres { schema, .. } if schema == "public"));
        assert!(matches!(&cfg.sources[1].kind, SourceKind::Bigquery { auth: BqAuth::None, .. }));
        assert_eq!(cfg.sources[1].max_export_rows, 100_000);
    }

    #[test]
    fn rejects_bad_ids_and_missing_env() {
        let base = "[storage]\npostgres_url = \"x\"\n[[source]]\nlabel = \"L\"\nkind = \"postgres\"\nurl = \"u\"\n";
        assert!(AppConfig::parse(&format!("{base}id = \"Bad Id\"")).is_err());
        assert!(AppConfig::parse("[storage]\npostgres_url = \"${WEBUI_TEST_UNSET_VAR}\"").is_err());
        assert!(AppConfig::parse("# use ${NAME} here\n[storage]\npostgres_url = \"x\"").is_ok());
    }

    #[test]
    fn shipped_config_parses() {
        let cfg = AppConfig::parse(&std::fs::read_to_string("config/app.toml").unwrap()).unwrap();
        assert!(cfg.sources.iter().any(|s| s.kind.name() == "postgres"));
        assert!(cfg.sources.iter().any(|s| s.kind.name() == "bigquery"));
    }
}
