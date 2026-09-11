//! The scheduled half of inlining a linked picture: reach the open internet,
//! decide, store. Runs as a CronJob with its own egress; the web pod has none.
//!
//! Takes what readers have asked for, in the order they asked, up to
//! `LINK_FETCH_BATCH` a run. Nothing here reads the archive: a link reaches this
//! queue because somebody was served a message containing it.

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

    let pool = db::connect(cfg.db_options.clone()).await?;
    db::ensure_schema(&pool).await?;

    // ⚠ A NAME, because the other end sees it. A fetch with no user agent reads
    // as a scraper, and this one is a person's chat client showing them a
    // picture they were sent.
    let client = reqwest::Client::builder()
        .user_agent("xinutec-messages/1.0 (link preview; one fetch per link, ever)")
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()?;

    let r = link_fetch::run(&pool, &client, &dir, link_fetch::Limits::default(), batch).await?;

    // ⚠ `still_wanted` is the number that means something. "stored 2" says a run
    // happened; a queue that keeps growing says readers are asking for more than
    // this cadence delivers, and a queue at zero says everything anyone has
    // looked at has been decided — which is the steady state this should sit in.
    tracing::info!(
        "took {} want(s); stored {}, not a picture {}, unreachable {}; {} still wanted",
        r.taken,
        r.stored,
        r.not_image,
        r.failed,
        r.still_wanted
    );
    Ok(())
}
