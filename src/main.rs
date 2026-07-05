//! messages — read-only viewer for the Signal + Google Chat archive. Loads
//! config, connects the shared `signal` DB, ensures its own sessions table,
//! serves. All logic lives in the `messages` library crate.

use anyhow::Result;
use messages::{config::Config, db, routes, state::AppState};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env()?;
    if cfg.allowed_users.is_empty() {
        anyhow::bail!("ALLOWED_USERS is empty — refusing to start (would deny everyone)");
    }
    tracing::info!("allow-list: {:?}", cfg.allowed_users);

    let pool = db::connect(cfg.db_options.clone()).await?;
    db::ensure_schema(&pool).await?;

    let http = reqwest::Client::builder().build()?;
    let bind_addr = cfg.bind_addr.clone();
    let app = routes::router(AppState::new(pool, cfg, http));

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("messages listening on {bind_addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
