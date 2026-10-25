//! Google BigQuery adapter over the REST API (jobs.query / getQueryResults, tables.get/list).
use super::config::{BqAuth, SourceConfig, SourceKind};
use super::source::{DataSource, Dialect, Rows};
use crate::model::Column;
use async_trait::async_trait;
use gcp_auth::TokenProvider;
use reqwest::{RequestBuilder, Url};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OnceCell;

const SCOPE: &str = "https://www.googleapis.com/auth/bigquery";

pub struct BigQuerySource {
    http: reqwest::Client,
    endpoint: String,
    project: String,
    dataset: String,
    location: Option<String>,
    timeout_ms: u64,
    auth: Option<Arc<dyn TokenProvider>>,
    // Each metadata lookup is an HTTP round trip, so cache them; schema changes need a restart.
    tables: OnceCell<Vec<String>>,
    columns: Mutex<HashMap<String, Vec<Column>>>,
}

impl BigQuerySource {
    pub async fn connect(cfg: &SourceConfig) -> Result<Self, String> {
        let SourceKind::Bigquery { project, dataset, location, endpoint, auth, timeout_ms } = &cfg.kind else {
            unreachable!("factory passes bigquery configs only")
        };
        let auth = match auth {
            BqAuth::Adc => Some(gcp_auth::provider().await.map_err(|e| format!("no GCP credentials: {e}"))?),
            BqAuth::None => None,
        };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms + 10_000))
            .build()
            .map_err(|e| e.to_string())?;
        let src = BigQuerySource {
            http,
            endpoint: endpoint.clone(),
            project: project.clone(),
            dataset: dataset.clone(),
            location: location.clone(),
            timeout_ms: *timeout_ms,
            auth,
            tables: OnceCell::new(),
            columns: Mutex::new(HashMap::new()),
        };
        src.list_tables().await?; // fail fast: checks credentials, project and dataset
        Ok(src)
    }

    /// `{endpoint}/bigquery/v2/projects/{project}/{segs...}` with every segment URL-encoded.
    fn url(&self, segs: &[&str]) -> Result<Url, String> {
        let mut u = Url::parse(&self.endpoint).map_err(|e| format!("bad BigQuery endpoint: {e}"))?;
        u.path_segments_mut()
            .map_err(|_| "bad BigQuery endpoint")?
            .pop_if_empty()
            .extend(["bigquery", "v2", "projects", &self.project])
            .extend(segs);
        Ok(u)
    }

    async fn send(&self, rb: RequestBuilder) -> Result<Value, String> {
        let rb = match &self.auth {
            Some(a) => rb.bearer_auth(a.token(&[SCOPE]).await.map_err(|e| format!("GCP token: {e}"))?.as_str()),
            None => rb,
        };
        let resp = rb.send().await.map_err(|e| format!("BigQuery: {e}"))?;
        let status = resp.status();
        let body: Value = resp.json().await.map_err(|e| format!("BigQuery: bad response ({status}): {e}"))?;
        if !status.is_success() {
            return Err(format!("BigQuery: {}", body["error"]["message"].as_str().unwrap_or(status.as_str())));
        }
        Ok(body)
    }
}

/// Rows from a jobs.query / getQueryResults response (`rows[].f[].v`).
pub fn parse_rows(resp: &Value) -> Rows {
    let Some(rows) = resp["rows"].as_array() else { return vec![] };
    rows.iter()
        .map(|r| {
            r["f"].as_array().map_or_else(Vec::new, |cells| {
                cells
                    .iter()
                    .map(|c| match &c["v"] {
                        Value::Null => None,
                        Value::String(s) => Some(s.clone()),
                        other => Some(other.to_string()),
                    })
                    .collect()
            })
        })
        .collect()
}

/// Column type as shown in the UI: `ARRAY<T>` for REPEATED fields, otherwise the field type.
pub fn field_type(field: &Value) -> String {
    let ty = field["type"].as_str().unwrap_or("STRING");
    if field["mode"].as_str() == Some("REPEATED") { format!("ARRAY<{ty}>") } else { ty.to_string() }
}

pub struct BqDialect;

impl Dialect for BqDialect {
    fn quote_ident(&self, name: &str) -> String {
        format!("`{}`", name.replace('\\', "\\\\").replace('`', "\\`"))
    }
    fn param(&self, n: usize) -> String {
        format!("@p{n}")
    }
    fn text(&self, col: &Column) -> String {
        let id = self.quote_ident(&col.name);
        match col.ty.as_str() {
            "STRING" => id,
            "BYTES" => format!("TO_BASE64({id})"),
            "GEOGRAPHY" => format!("ST_ASTEXT({id})"),
            t if t.starts_with("ARRAY<") || matches!(t, "RECORD" | "STRUCT" | "JSON" | "RANGE") => {
                format!("TO_JSON_STRING({id})")
            }
            _ => format!("CAST({id} AS STRING)"),
        }
    }
    fn text_expr(&self, expr: &str) -> String {
        format!("CAST({expr} AS STRING)")
    }
    fn like(&self, lhs: &str, pattern: &str, cs: bool) -> String {
        if cs { format!("{lhs} LIKE {pattern}") } else { format!("LOWER({lhs}) LIKE LOWER({pattern})") }
    }
    fn regex(&self, lhs: &str, pattern: &str, cs: bool) -> String {
        // RE2 syntax; `(?i)` turns on case-insensitive matching.
        if cs { format!("REGEXP_CONTAINS({lhs}, {pattern})") } else { format!("REGEXP_CONTAINS({lhs}, CONCAT('(?i)', {pattern}))") }
    }
    fn compare_type(&self, col: &Column) -> Option<String> {
        Some(
            match col.ty.as_str() {
                "INTEGER" | "INT64" => "INT64",
                "FLOAT" | "FLOAT64" => "FLOAT64",
                "BOOLEAN" | "BOOL" => "BOOL",
                t @ ("NUMERIC" | "BIGNUMERIC" | "STRING" | "DATE" | "DATETIME" | "TIME" | "TIMESTAMP") => t,
                _ => return None,
            }
            .to_string(),
        )
    }
    fn sortable(&self, col: &Column) -> bool {
        !(col.ty.starts_with("ARRAY<") || matches!(col.ty.as_str(), "RECORD" | "STRUCT" | "JSON" | "GEOGRAPHY" | "RANGE"))
    }
}

#[async_trait]
impl DataSource for BigQuerySource {
    fn dialect(&self) -> &dyn Dialect {
        &BqDialect
    }

    fn table_ref(&self, table: &str) -> String {
        BqDialect.quote_ident(&format!("{}.{}.{}", self.project, self.dataset, table))
    }

    async fn list_tables(&self) -> Result<Vec<String>, String> {
        self.tables
            .get_or_try_init(|| async {
                let mut names = Vec::new();
                let mut token: Option<String> = None;
                loop {
                    let mut url = self.url(&["datasets", &self.dataset, "tables"])?;
                    url.query_pairs_mut().append_pair("maxResults", "1000");
                    if let Some(t) = &token {
                        url.query_pairs_mut().append_pair("pageToken", t);
                    }
                    let resp = self.send(self.http.get(url)).await?;
                    for t in resp["tables"].as_array().into_iter().flatten() {
                        if let Some(id) = t["tableReference"]["tableId"].as_str() {
                            names.push(id.to_string());
                        }
                    }
                    token = resp["nextPageToken"].as_str().map(str::to_owned);
                    if token.is_none() {
                        names.sort();
                        return Ok(names);
                    }
                }
            })
            .await
            .cloned()
    }

    async fn columns(&self, table: &str) -> Result<Vec<Column>, String> {
        if let Some(c) = self.columns.lock().unwrap().get(table) {
            return Ok(c.clone());
        }
        let resp = self.send(self.http.get(self.url(&["datasets", &self.dataset, "tables", table])?)).await?;
        let cols: Vec<Column> = resp["schema"]["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| Column { name: f["name"].as_str().unwrap_or_default().to_string(), ty: field_type(f) })
            .collect();
        self.columns.lock().unwrap().insert(table.to_string(), cols.clone());
        Ok(cols)
    }

    async fn query(&self, sql: &str, params: &[String]) -> Result<Rows, String> {
        let query_params: Vec<Value> = params
            .iter()
            .enumerate()
            .map(|(i, v)| {
                json!({
                    "name": format!("p{}", i + 1),
                    "parameterType": { "type": "STRING" },
                    "parameterValue": { "value": v },
                })
            })
            .collect();
        let mut body = json!({
            "query": sql,
            "useLegacySql": false,
            "parameterMode": "NAMED",
            "queryParameters": query_params,
            "timeoutMs": self.timeout_ms,
            "jobTimeoutMs": self.timeout_ms,
        });
        if let Some(l) = &self.location {
            body["location"] = json!(l);
        }
        let mut resp = self.send(self.http.post(self.url(&["queries"])?).json(&body)).await?;
        let mut rows = Rows::new();
        // Poll until the job completes, then follow pageTokens until every row is read.
        loop {
            let complete = resp["jobComplete"].as_bool().unwrap_or(false);
            if complete {
                rows.extend(parse_rows(&resp));
            }
            let page_token = resp["pageToken"].as_str().map(str::to_owned);
            if complete && page_token.is_none() {
                return Ok(rows);
            }
            let job = resp["jobReference"]["jobId"].as_str().ok_or("BigQuery: response without jobId")?.to_owned();
            let location = resp["jobReference"]["location"].as_str().map(str::to_owned).or(self.location.clone());
            let mut url = self.url(&["queries", &job])?;
            {
                let mut q = url.query_pairs_mut();
                q.append_pair("timeoutMs", &self.timeout_ms.to_string());
                if let Some(l) = &location {
                    q.append_pair("location", l);
                }
                if let (true, Some(t)) = (complete, &page_token) {
                    q.append_pair("pageToken", t);
                }
            }
            resp = self.send(self.http.get(url)).await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rest_rows_and_field_types() {
        let resp = json!({
            "jobComplete": true,
            "rows": [ { "f": [ { "v": "1" }, { "v": null } ] }, { "f": [ { "v": "2" }, { "v": "x" } ] } ]
        });
        assert_eq!(parse_rows(&resp), vec![vec![Some("1".into()), None], vec![Some("2".into()), Some("x".into())]]);
        assert_eq!(parse_rows(&json!({ "jobComplete": true })), Rows::new());
        assert_eq!(field_type(&json!({ "name": "t", "type": "STRING", "mode": "REPEATED" })), "ARRAY<STRING>");
        assert_eq!(field_type(&json!({ "name": "n", "type": "INTEGER", "mode": "NULLABLE" })), "INTEGER");
    }

    #[test]
    fn table_refs_and_text_rendering() {
        assert_eq!(BqDialect.quote_ident("my-proj.ds.t`x"), "`my-proj.ds.t\\`x`");
        let c = |ty: &str| Column { name: "c".into(), ty: ty.into() };
        assert_eq!(BqDialect.text(&c("INTEGER")), "CAST(`c` AS STRING)");
        assert_eq!(BqDialect.text(&c("RECORD")), "TO_JSON_STRING(`c`)");
        assert!(!BqDialect.sortable(&c("ARRAY<INT64>")));
        assert_eq!(BqDialect.compare_type(&c("TIMESTAMP")).as_deref(), Some("TIMESTAMP"));
    }
}
