//! Server-only code (compiled with the `ssr` feature).
pub mod bigquery;
pub mod config;
pub mod export;
pub mod postgres;
pub mod query;
pub mod source;
pub mod store;

/// Loads `config/app.toml`, builds every data source through the factory, and opens storage.
pub async fn init() {
    let cfg = config::AppConfig::load().unwrap_or_else(|e| panic!("cannot load config: {e}"));
    source::init(&cfg.sources).await;
    if let Err(e) = store::init(&cfg.storage.postgres_url).await {
        eprintln!("saved queries disabled: {e}");
    }
}
