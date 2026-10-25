//! Saved / shared searches, stored in the `[storage]` PostgreSQL database.
use super::postgres::{db_err, pool};
use crate::model::{SavedQuery, SearchRequest};
use deadpool_postgres::Pool;
use std::sync::OnceLock;

static POOL: OnceLock<Pool> = OnceLock::new();

// gen_random_uuid() is built into PostgreSQL 13+.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS saved_queries (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name        text NOT NULL,
    source      text NOT NULL,
    table_name  text NOT NULL,
    request     jsonb NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS saved_queries_created_at ON saved_queries (created_at DESC);";

pub async fn init(url: &str) -> Result<(), String> {
    let pool = pool(url, 5_000, 4)?;
    let client = pool.get().await.map_err(|e| format!("storage: {e}"))?;
    client.batch_execute(SCHEMA).await.map_err(db_err)?;
    drop(client);
    let _ = POOL.set(pool);
    Ok(())
}

async fn client() -> Result<deadpool_postgres::Object, String> {
    POOL.get()
        .ok_or("saved queries are disabled (storage database unavailable)")?
        .get()
        .await
        .map_err(|e| format!("storage: {e}"))
}

const COLUMNS: &str = "id::text, name, to_char(created_at, 'YYYY-MM-DD HH24:MI'), request::text";

fn row_to_saved(r: &tokio_postgres::Row) -> Result<SavedQuery, String> {
    Ok(SavedQuery {
        id: r.get(0),
        name: r.get(1),
        created_at: r.get(2),
        request: serde_json::from_str(r.get::<_, &str>(3)).map_err(|e| format!("stored query is corrupt: {e}"))?,
    })
}

pub async fn save(name: &str, req: &SearchRequest) -> Result<SavedQuery, String> {
    let name: String = name.trim().chars().take(120).collect();
    let name = if name.is_empty() { "Untitled search".to_string() } else { name };
    let json = serde_json::to_string(req).map_err(|e| e.to_string())?;
    let row = client()
        .await?
        .query_one(
            &format!(
                "INSERT INTO saved_queries (name, source, table_name, request)
                 VALUES ($1, $2, $3, $4::text::jsonb) RETURNING {COLUMNS}"
            ),
            &[&name, &req.source, &req.table, &json],
        )
        .await
        .map_err(db_err)?;
    row_to_saved(&row)
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() })
}

pub async fn load(id: &str) -> Result<SavedQuery, String> {
    if !is_uuid(id) {
        return Err("saved query not found".into());
    }
    let row = client()
        .await?
        .query_opt(&format!("SELECT {COLUMNS} FROM saved_queries WHERE id = $1::text::uuid"), &[&id])
        .await
        .map_err(db_err)?
        .ok_or("saved query not found")?;
    row_to_saved(&row)
}

pub async fn recent(limit: i64) -> Result<Vec<SavedQuery>, String> {
    let rows = client()
        .await?
        .query(&format!("SELECT {COLUMNS} FROM saved_queries ORDER BY created_at DESC LIMIT $1"), &[&limit])
        .await
        .map_err(db_err)?;
    rows.iter().map(row_to_saved).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn uuid_check() {
        assert!(super::is_uuid("3f2b8c1e-9a4d-4c2b-8e1f-0a1b2c3d4e5f"));
        assert!(!super::is_uuid("3f2b8c1e-9a4d-4c2b-8e1f-0a1b2c3d4e5"));
        assert!(!super::is_uuid("'; drop table saved_queries; --xxxxxxxxx"));
    }
}
