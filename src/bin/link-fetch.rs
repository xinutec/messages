//! The scheduled half of inlining a linked picture: reach the open internet,
//! decide, store. Runs as a CronJob with its own egress; the web pod has none.
//!
//! Deliberately boring and bounded — a run asks about at most `LINK_FETCH_BATCH`
//! links and then stops, so a first pass over years of archive is spread across
//! runs rather than arriving at somebody's server all at once.

use std::path::PathBuf;

use anyhow::Result;
use messages::{config::Config, db, link_fetch};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env()?;
    let dir =
        PathBuf::from(std::env::var("LINK_IMAGES_DIR").unwrap_or_else(|_| "/link-images".into()));
    let batch: usize = std::env::var("LINK_FETCH_BATCH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25);
    let scan: u32 = std::env::var("LINK_FETCH_SCAN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);

    let pool = db::connect(cfg.db_options.clone()).await?;
    db::ensure_schema(&pool).await?;

    // ⚠ A NAME, because the other end sees it. A fetch with no user agent reads
    // as a scraper, and this one is a person's chat client showing them a
    // picture they were sent.
    let client = reqwest::Client::builder()
        .user_agent("xinutec-messages/1.0 (link preview; one fetch per link, ever)")
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()?;

    let urls = link_fetch::undecided_urls(&pool, scan, batch).await?;
    tracing::info!(
        "{} undecided link(s) in the newest {scan} lines",
        urls.len()
    );

    let limits = link_fetch::Limits::default();
    let (mut ok, mut not_image, mut failed) = (0u32, 0u32, 0u32);
    for url in &urls {
        match link_fetch::resolve_one(&pool, &client, &dir, url, limits).await? {
            link_fetch::Outcome::Ok => ok += 1,
            link_fetch::Outcome::NotImage => not_image += 1,
            link_fetch::Outcome::Failed => failed += 1,
        }
    }
    tracing::info!("stored {ok}, not a picture {not_image}, unreachable {failed}");
    Ok(())
}
