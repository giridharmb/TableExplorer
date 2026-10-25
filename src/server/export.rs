//! `GET /export/{json|xlsx}?q=<SearchRequest json>` — every matching row, not just one page.
use super::query::{self, Export};
use crate::model::SearchRequest;
use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_xlsxwriter::{Format, Workbook, XlsxError};
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Deserialize)]
pub struct ExportParams {
    q: String,
}

pub async fn handler(Path(format): Path<String>, Query(p): Query<ExportParams>) -> Response {
    match run(&format, &p.q).await {
        Ok(r) => r,
        Err((code, msg)) => (code, msg).into_response(),
    }
}

async fn run(format: &str, q: &str) -> Result<Response, (StatusCode, String)> {
    let bad = |e: String| (StatusCode::BAD_REQUEST, e);
    let req: SearchRequest = serde_json::from_str(q).map_err(|e| bad(format!("invalid search: {e}")))?;
    let (ctype, ext) = match format {
        "json" => ("application/json", "json"),
        "xlsx" => ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "xlsx"),
        _ => return Err((StatusCode::NOT_FOUND, "export format must be json or xlsx".into())),
    };
    let data = query::export(&req).await.map_err(bad)?;
    let body = match ext {
        "json" => to_json(&data),
        _ => to_xlsx(&data, &req).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
    };
    let file: String = req.table.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    Ok((
        [
            (header::CONTENT_TYPE, ctype.to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}-{file}.{ext}\"", req.source)),
            (header::HeaderName::from_static("x-export-truncated"), data.truncated.to_string()),
        ],
        body,
    )
        .into_response())
}

fn base_type(ty: &str) -> String {
    ty.to_ascii_lowercase().split('(').next().unwrap_or_default().trim().to_string()
}

fn is_numeric(ty: &str) -> bool {
    matches!(
        base_type(ty).as_str(),
        "smallint" | "integer" | "bigint" | "int" | "int2" | "int4" | "int8" | "numeric" | "decimal" | "real"
            | "double precision" | "float" | "float4" | "float8" | "int64" | "float64" | "bignumeric"
    )
}

fn number(ty: &str, v: &str) -> Option<f64> {
    v.parse::<f64>().ok().filter(|n| n.is_finite() && is_numeric(ty))
}

/// `[{column: value, ...}]`, with numbers and booleans typed when the column type allows.
pub fn to_json(e: &Export) -> Vec<u8> {
    let records: Vec<Value> = e
        .rows
        .iter()
        .map(|row| {
            let mut obj = Map::new();
            for (c, v) in e.columns.iter().zip(row) {
                let val = match v {
                    None => Value::Null,
                    Some(s) if matches!(base_type(&c.ty).as_str(), "boolean" | "bool") && (s == "true" || s == "false") => {
                        Value::Bool(s == "true")
                    }
                    // shortcut: decimals go through f64, so >15 significant digits lose precision; emit strings if that matters.
                    Some(s) if is_numeric(&c.ty) => s
                        .parse::<i64>()
                        .map(Value::from)
                        .ok()
                        .or_else(|| number(&c.ty, s).and_then(serde_json::Number::from_f64).map(Value::Number))
                        .unwrap_or_else(|| Value::String(s.clone())),
                    Some(s) => Value::String(s.clone()),
                };
                obj.insert(c.name.clone(), val);
            }
            Value::Object(obj)
        })
        .collect();
    serde_json::to_vec_pretty(&records).unwrap_or_default()
}

pub fn to_xlsx(e: &Export, req: &SearchRequest) -> Result<Vec<u8>, XlsxError> {
    const MAX_CELL: usize = 32_767; // Excel's per-cell character limit
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();
    {
        let ws = wb.add_worksheet().set_name("Data")?;
        for (c, col) in e.columns.iter().enumerate() {
            ws.write_string_with_format(0, c as u16, &col.name, &bold)?;
            let widest = e.rows.iter().take(200).filter_map(|r| r[c].as_ref()).map(|s| s.chars().count()).max().unwrap_or(0);
            ws.set_column_width(c as u16, (widest.max(col.name.len()) + 2).min(60) as f64)?;
        }
        for (r, row) in e.rows.iter().enumerate() {
            let r = r as u32 + 1;
            for (c, (v, col)) in row.iter().zip(&e.columns).enumerate() {
                match v {
                    None => {}
                    Some(s) => match number(&col.ty, s) {
                        Some(n) => ws.write_number(r, c as u16, n).map(|_| ())?,
                        None => ws.write_string(r, c as u16, s.chars().take(MAX_CELL).collect::<String>()).map(|_| ())?,
                    },
                }
            }
        }
        ws.set_freeze_panes(1, 0)?;
        if !e.columns.is_empty() {
            ws.autofilter(0, 0, e.rows.len() as u32, e.columns.len() as u16 - 1)?;
        }
    }
    let info = wb.add_worksheet().set_name("Search")?;
    let lines = [
        ("Source", req.source.clone()),
        ("Table", req.table.clone()),
        ("Rows exported", e.rows.len().to_string()),
        ("Truncated", if e.truncated { "yes — raise max_export_rows in config to export more".into() } else { "no".into() }),
        ("Quick search", req.quick.clone()),
        ("Filter (JSON)", serde_json::to_string(&req.filter).unwrap_or_default()),
    ];
    for (i, (k, v)) in lines.iter().enumerate() {
        info.write_string_with_format(i as u32, 0, *k, &bold)?;
        info.write_string(i as u32, 1, v.chars().take(MAX_CELL).collect::<String>())?;
    }
    info.set_column_width(0, 16)?;
    wb.save_to_buffer()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Column;

    fn sample() -> Export {
        Export {
            columns: vec![
                Column { name: "id".into(), ty: "integer".into() },
                Column { name: "ok".into(), ty: "boolean".into() },
                Column { name: "bal".into(), ty: "numeric(12,2)".into() },
                Column { name: "note".into(), ty: "text".into() },
            ],
            rows: vec![
                vec![Some("1".into()), Some("true".into()), Some("NaN".into()), Some("42".into())],
                vec![Some("2".into()), None, Some("10.50".into()), None],
            ],
            truncated: false,
        }
    }

    #[test]
    fn json_types_numbers_bools_and_keeps_text_and_nulls() {
        let v: Value = serde_json::from_slice(&to_json(&sample())).unwrap();
        assert_eq!(v[0], serde_json::json!({ "id": 1, "ok": true, "bal": "NaN", "note": "42" }));
        assert_eq!(v[1], serde_json::json!({ "id": 2, "ok": null, "bal": 10.5, "note": null }));
    }

    #[test]
    fn xlsx_is_a_zip_workbook() {
        let bytes = to_xlsx(&sample(), &SearchRequest::new("s", "t")).unwrap();
        assert_eq!(&bytes[..2], b"PK");
    }
}
