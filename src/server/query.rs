//! Engine-independent search: filter tree → SQL via a `Dialect`, then count + page or export.
use super::source::{self, DataSource, Dialect, Rows};
use crate::model::*;
use std::sync::Arc;
use std::time::Instant;

/// Collects bind parameters; every parameter is a string, cast in SQL where needed.
#[derive(Default)]
pub struct Params(pub Vec<String>);

impl Params {
    fn bind(&mut self, d: &dyn Dialect, v: String) -> String {
        self.0.push(v);
        d.param(self.0.len())
    }
}

fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn glob_to_like(s: &str) -> String {
    like_escape(s).replace('*', "%").replace('?', "_")
}

pub fn build_where(f: &Filter, cols: &[Column], d: &dyn Dialect, p: &mut Params) -> Result<String, String> {
    match f {
        Filter::Group { join, negate, children } => {
            let mut parts = Vec::new();
            for c in children {
                let s = build_where(c, cols, d, p)?;
                if s != "TRUE" {
                    parts.push(s);
                }
            }
            let sep = if *join == Join::And { " AND " } else { " OR " };
            let body = if parts.is_empty() { "TRUE".to_string() } else { format!("({})", parts.join(sep)) };
            Ok(if *negate { format!("NOT COALESCE({body}, FALSE)") } else { body })
        }
        Filter::Rule { column, op, values, case_sensitive: cs } => {
            let col = cols
                .iter()
                .find(|c| &c.name == column)
                .ok_or_else(|| format!("unknown column {column:?}"))?;
            let id = d.quote_ident(&col.name);
            let txt = d.text(col);
            match op {
                Op::IsNull => return Ok(format!("{id} IS NULL")),
                Op::NotNull => return Ok(format!("{id} IS NOT NULL")),
                Op::IsEmpty => return Ok(format!("({id} IS NULL OR TRIM({txt}) IN ('', '{{}}', '[]'))")),
                _ => {}
            }
            let mut conds = Vec::new();
            for v in values.iter().filter(|v| !v.is_empty()) {
                conds.push(match op {
                    Op::Contains => d.like(&txt, &p.bind(d, format!("%{}%", like_escape(v))), *cs),
                    Op::StartsWith => d.like(&txt, &p.bind(d, format!("{}%", like_escape(v))), *cs),
                    Op::EndsWith => d.like(&txt, &p.bind(d, format!("%{}", like_escape(v))), *cs),
                    Op::Wildcard => d.like(&txt, &p.bind(d, glob_to_like(v)), *cs),
                    Op::Equals | Op::NotEquals => d.equals(&txt, &p.bind(d, v.clone()), *cs),
                    Op::Regex | Op::NotRegex => d.regex(&txt, &p.bind(d, v.clone()), *cs),
                    // Comparisons use the column's real type, e.g. numeric or timestamp.
                    Op::Gt | Op::Gte | Op::Lt | Op::Lte => {
                        let ty = d.compare_type(col).ok_or_else(|| {
                            format!("column {:?} ({}) can't be compared with < or >", col.name, col.ty)
                        })?;
                        let sym = match op { Op::Gt => ">", Op::Gte => ">=", Op::Lt => "<", _ => "<=" };
                        format!("{id} {sym} CAST({} AS {ty})", p.bind(d, v.clone()))
                    }
                    Op::IsNull | Op::NotNull | Op::IsEmpty => unreachable!(),
                });
            }
            if conds.is_empty() {
                return Ok("TRUE".into());
            }
            let any = format!("({})", conds.join(" OR "));
            // NULL-safe negation: NULL cells count as "not equal".
            Ok(if matches!(op, Op::NotEquals | Op::NotRegex) { format!("NOT COALESCE({any}, FALSE)") } else { any })
        }
    }
}

/// Validated source/table, its columns, and the compiled `FROM … WHERE … ORDER BY …`.
struct Prepared {
    entry: &'static source::Entry,
    src: Arc<dyn DataSource>,
    cols: Vec<Column>,
    from: String,
    order: String,
    params: Params,
}

impl Prepared {
    fn select(&self) -> String {
        let d = self.src.dialect();
        // `__cN` aliases keep ORDER BY bound to the typed column, not its text rendering.
        let list: Vec<String> = self.cols.iter().enumerate().map(|(i, c)| format!("{} AS __c{i}", d.text(c))).collect();
        list.join(", ")
    }
}

async fn prepare(req: &SearchRequest) -> Result<Prepared, String> {
    let entry = source::get(&req.source)?;
    let src = entry.source()?.clone();
    if !entry.tables().await?.contains(&req.table) {
        return Err(format!("table {:?} is not exposed by source {:?}", req.table, req.source));
    }
    let cols = src.columns(&req.table).await?;
    let d = src.dialect();
    let mut params = Params::default();
    let mut wh = build_where(&req.filter, &cols, d, &mut params)?;
    let quick = req.quick.trim();
    if !quick.is_empty() {
        let ph = params.bind(d, format!("%{}%", like_escape(quick)));
        let any: Vec<String> = cols.iter().map(|c| d.like(&d.text(c), &ph, false)).collect();
        wh = format!("{wh} AND ({})", any.join(" OR "));
    }
    let from = format!("FROM {} WHERE {wh}", src.table_ref(&req.table));

    let (sort_col, desc) = match &req.sort {
        Some((c, desc)) => (cols.iter().find(|x| &x.name == c), *desc),
        None => (None, false),
    };
    let sort_col = sort_col.or(cols.first()).ok_or("table has no columns")?;
    let sort_expr = if d.sortable(sort_col) { d.quote_ident(&sort_col.name) } else { d.text(sort_col) };
    let order = format!(" ORDER BY {sort_expr} {} NULLS LAST", if desc { "DESC" } else { "ASC" });
    Ok(Prepared { entry, src, cols, from, order, params })
}

pub async fn search(req: SearchRequest) -> Result<SearchPage, String> {
    let start = Instant::now();
    let p = prepare(&req).await?;
    let d = p.src.dialect();
    let page_size = req.page_size.clamp(1, 200);
    let page = req.page.max(1);
    let limit = format!(" LIMIT {page_size} OFFSET {}", (page as u64 - 1) * page_size as u64);

    let count_sql = format!("SELECT {} {}", d.text_expr("COUNT(*)"), p.from);
    let data_sql = format!("SELECT {} {}{}{limit}", p.select(), p.from, p.order);
    let (count, rows) = tokio::try_join!(p.src.query(&count_sql, &p.params.0), p.src.query(&data_sql, &p.params.0))?;
    let total = count
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|s| s.parse().ok())
        .ok_or("count query returned no number")?;

    let shown: Vec<String> = p.params.0.iter().enumerate().map(|(i, v)| format!("{} = {v:?}", d.param(i + 1))).collect();
    let sql = format!(
        "SELECT * {}{}{limit}\n-- params: {}",
        p.from,
        p.order,
        if shown.is_empty() { "none".into() } else { shown.join(", ") }
    );
    Ok(SearchPage {
        columns: p.cols,
        rows,
        total,
        page,
        page_size,
        sql,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

pub struct Export {
    pub columns: Vec<Column>,
    pub rows: Rows,
    /// True when more rows matched than the source's `max_export_rows`.
    pub truncated: bool,
}

/// Every matching row (ignoring paging), capped at the source's `max_export_rows`.
pub async fn export(req: &SearchRequest) -> Result<Export, String> {
    let p = prepare(req).await?;
    let max = p.entry.cfg.max_export_rows;
    let sql = format!("SELECT {} {}{} LIMIT {}", p.select(), p.from, p.order, max + 1);
    let mut rows = p.src.query(&sql, &p.params.0).await?;
    let truncated = rows.len() as u64 > max;
    rows.truncate(max as usize);
    Ok(Export { columns: p.cols, rows, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{bigquery::BqDialect, postgres::PgDialect};

    fn col(name: &str, ty: &str) -> Column {
        Column { name: name.into(), ty: ty.into() }
    }

    fn rule(column: &str, op: Op, values: &[&str]) -> Filter {
        Filter::Rule { column: column.into(), op, values: values.iter().map(|s| s.to_string()).collect(), case_sensitive: false }
    }

    fn sample() -> Filter {
        Filter::Group {
            join: Join::And,
            negate: false,
            children: vec![
                rule("name", Op::Wildcard, &["a*_x?"]),
                Filter::Group {
                    join: Join::Or,
                    negate: true,
                    children: vec![rule("age", Op::Gt, &["30", "40"]), rule("name", Op::IsNull, &[])],
                },
                rule("name", Op::Contains, &[]), // no values → ignored
            ],
        }
    }

    #[test]
    fn postgres_nested_groups_with_bound_params() {
        let cols = [col("name", "text"), col("age", "integer")];
        let mut p = Params::default();
        let sql = build_where(&sample(), &cols, &PgDialect, &mut p).unwrap();
        assert_eq!(
            sql,
            "((\"name\"::text ILIKE $1::text) AND NOT COALESCE(((\"age\" > CAST($2::text AS integer) OR \"age\" > CAST($3::text AS integer)) OR \"name\" IS NULL), FALSE))"
        );
        assert_eq!(p.0, vec!["a%\\_x_", "30", "40"]);
    }

    #[test]
    fn bigquery_same_tree_uses_named_params_and_bq_types() {
        let cols = [col("name", "STRING"), col("age", "INTEGER")];
        let mut p = Params::default();
        let sql = build_where(&sample(), &cols, &BqDialect, &mut p).unwrap();
        assert_eq!(
            sql,
            "((LOWER(`name`) LIKE LOWER(@p1)) AND NOT COALESCE(((`age` > CAST(@p2 AS INT64) OR `age` > CAST(@p3 AS INT64)) OR `name` IS NULL), FALSE))"
        );
        let mut p = Params::default();
        let re = build_where(&rule("tags", Op::Regex, &["^v"]), &[col("tags", "ARRAY<STRING>")], &BqDialect, &mut p).unwrap();
        assert_eq!(re, "(REGEXP_CONTAINS(TO_JSON_STRING(`tags`), CONCAT('(?i)', @p1)))");
    }

    #[test]
    fn rejects_unknown_columns_and_uncomparable_types() {
        let mut p = Params::default();
        assert!(build_where(&rule("x\"; drop", Op::Equals, &["1"]), &[col("a", "text")], &PgDialect, &mut p).is_err());
        assert!(build_where(&rule("s", Op::Gt, &["1"]), &[col("s", "RECORD")], &BqDialect, &mut p).is_err());
        assert_eq!(PgDialect.quote_ident("a\"b"), "\"a\"\"b\"");
    }
}
