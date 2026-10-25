//! Types shared by the server (SSR) and the WASM client.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Join {
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Contains,
    Equals,
    NotEquals,
    StartsWith,
    EndsWith,
    Wildcard,
    Regex,
    NotRegex,
    Gt,
    Gte,
    Lt,
    Lte,
    IsNull,
    NotNull,
    IsEmpty,
}

impl Op {
    pub const ALL: [Op; 15] = [
        Op::Contains,
        Op::Equals,
        Op::NotEquals,
        Op::StartsWith,
        Op::EndsWith,
        Op::Wildcard,
        Op::Regex,
        Op::NotRegex,
        Op::Gt,
        Op::Gte,
        Op::Lt,
        Op::Lte,
        Op::IsNull,
        Op::NotNull,
        Op::IsEmpty,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Op::Contains => "contains",
            Op::Equals => "equals (exact)",
            Op::NotEquals => "not equals",
            Op::StartsWith => "starts with",
            Op::EndsWith => "ends with",
            Op::Wildcard => "pattern ( * ? )",
            Op::Regex => "matches regex",
            Op::NotRegex => "not matches regex",
            Op::Gt => ">",
            Op::Gte => "≥",
            Op::Lt => "<",
            Op::Lte => "≤",
            Op::IsNull => "is NULL",
            Op::NotNull => "is not NULL",
            Op::IsEmpty => "is empty / blank",
        }
    }

    pub fn key(self) -> String {
        format!("{self:?}")
    }

    pub fn from_key(s: &str) -> Option<Op> {
        Op::ALL.into_iter().find(|o| o.key() == s)
    }

    pub fn needs_value(self) -> bool {
        !matches!(self, Op::IsNull | Op::NotNull | Op::IsEmpty)
    }
}

/// A nested boolean filter tree. A rule's `values` are OR'ed ("any of").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Filter {
    Rule {
        column: String,
        op: Op,
        values: Vec<String>,
        case_sensitive: bool,
    },
    Group {
        join: Join,
        negate: bool,
        children: Vec<Filter>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchRequest {
    /// Data source id from the config file (e.g. "local_pg", "bq_sales").
    pub source: String,
    pub table: String,
    pub filter: Filter,
    pub quick: String,
    /// (column, descending)
    pub sort: Option<(String, bool)>,
    pub page: u32,
    pub page_size: u32,
}

impl SearchRequest {
    pub fn new(source: impl Into<String>, table: impl Into<String>) -> Self {
        SearchRequest {
            source: source.into(),
            table: table.into(),
            filter: Filter::Group { join: Join::And, negate: false, children: vec![] },
            quick: String::new(),
            sort: None,
            page: 1,
            page_size: 25,
        }
    }
}

/// A configured data source as shown in the UI's table picker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub id: String,
    pub label: String,
    /// "postgres" | "bigquery"
    pub kind: String,
    pub tables: Vec<String>,
    /// Set when the source could not be reached (bad credentials, DB down, ...).
    pub error: Option<String>,
}

/// A search persisted in the storage database, shared as `/s/<id>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedQuery {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub request: SearchRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub ty: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchPage {
    pub columns: Vec<Column>,
    /// Every value is rendered as Postgres text; `None` is SQL NULL.
    pub rows: Vec<Vec<Option<String>>>,
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
    pub sql: String,
    pub elapsed_ms: f64,
}
