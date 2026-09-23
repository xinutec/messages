//! messages — viewer for the Signal, Google Chat, IRC and Telegram archive, which
//! can also send on IRC through irssi. Loads config, connects the shared `signal`
//! database, ensures its own tables, serves.

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

    // Sending is optional: any failure to prepare it is logged as an error and
    // leaves the archive readable, as a missing key does.
    let irc = match &cfg.irc_send {
        Some(c) => match IrcSender::prepare(c).await {
            Ok(sender) => sender,
            Err(e) => {
                tracing::error!("IRC send setup failed; sending is disabled: {e:#}");
                None
            }
        },
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
