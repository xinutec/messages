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

    let before = link_fetch::progress(&pool).await?;
    let r = link_fetch::run(
        &pool,
        &client,
        &dir,
        link_fetch::Limits::default(),
        batch,
        scan,
    )
    .await?;

    // ⚠ The WATERMARKS are the line worth printing. "stored 4" says a run
    // happened; `low` moving says the backfill is getting somewhere, and `low`
    // standing still across runs is the only symptom that the archive is not
    // being walked — which is exactly how the first version of this went
    // unnoticed until someone asked about a day in 2025.
    tracing::info!(
        "examined {} new + {} old line(s); stored {}, not a picture {}, unreachable {}",
        r.examined_new,
        r.examined_old,
        r.stored,
        r.not_image,
        r.failed
    );
    if let Some(after) = r.progress {
        if after.low == link_fetch::BACKFILL_DONE {
            tracing::info!(
                "progress: high {} -> {}; the backfill has reached the beginning of the archive",
                before.high,
                after.high
            );
        } else {
            tracing::info!(
                "progress: high {} -> {}, low {} -> {}; {} older line(s) still to examine",
                before.high,
                after.high,
                before.low,
                after.low,
                (after.low - 1).max(0)
            );
        }
    }
    Ok(())
}
