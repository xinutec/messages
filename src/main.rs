//! messages — viewer for the Signal, Google Chat and IRC archive, and the one
//! place that sends: IRC only, through irssi. Loads config, connects the shared
//! `signal` DB, ensures its own sessions table, serves. All logic lives in the
//! `messages` library crate.

use anyhow::Result;
use messages::{config::Config, db, irc_send::IrcSender, routes, state::AppState};
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

    // Sending is a capability, not a requirement: with no key mounted the app
    // still serves the archive and refuses every send. Failing to boot here
    // would take a working viewer down over something it does not need to read.
    let irc = match &cfg.irc_send {
        Some(c) => IrcSender::prepare(c).await?,
        None => None,
    };
    tracing::info!(
        "IRC sending {}",
        if irc.is_some() { "enabled" } else { "disabled" }
    );

    let http = reqwest::Client::builder().build()?;
    let bind_addr = cfg.bind_addr.clone();
    let app = routes::router(AppState::new(pool, cfg, http, irc));

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("messages listening on {bind_addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
