//! MariaDB pool. This app is a READ-ONLY consumer of the shared `signal`
//! database — the Signal tables are owned by the signal ingester's migrations
//! and the `gchat_*` tables by import_gchat.py. The only table this app owns is
//! its own `sessions` and `link_images`, created here on boot (CREATE TABLE IF
//! NOT EXISTS), kept deliberately out of any cross-app migration framework.

use anyhow::{Context, Result};
use sqlx::MySqlPool;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};

pub async fn connect(options: MySqlConnectOptions) -> Result<MySqlPool> {
    let pool = MySqlPoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .context("connecting to MariaDB")?;
    Ok(pool)
}

/// Create the app's own tables if absent. Idempotent.
pub async fn ensure_schema(pool: &MySqlPool) -> Result<()> {
    sqlx::query(
        r"CREATE TABLE IF NOT EXISTS sessions (
            id           CHAR(64)     NOT NULL PRIMARY KEY,
            user_id      VARCHAR(255) NOT NULL,
            display_name VARCHAR(255) NOT NULL,
            expires_at   DATETIME     NOT NULL,
            INDEX idx_sessions_expires (expires_at)
        ) DEFAULT CHARSET=utf8mb4",
    )
    .execute(pool)
    .await
    .context("creating sessions table")?;

    // What a link in a message turned out to be, keyed by the link itself: the
    // same picture posted in three channels is fetched once.
    //
    // ⚠ **THE REFUSALS ARE ROWS TOO, and that is the point of `state`.** A link
    // that is not a picture must be remembered as not a picture, or every pass
    // of the fetcher asks a stranger's server about it again — 173 links in one
    // channel alone, most of them GitHub and YouTube. Somebody else's server
    // should hear from us once per link, not once per run.
    sqlx::query(
        r"CREATE TABLE IF NOT EXISTS link_images (
            url_hash     CHAR(64)     NOT NULL PRIMARY KEY,
            url          TEXT         NOT NULL,
            state        ENUM('ok','not_image','failed') NOT NULL,
            content_type VARCHAR(128) NULL,
            size_bytes   BIGINT       NULL,
            stored_name  VARCHAR(80)  NULL,
            note         VARCHAR(255) NULL,
            fetched_at   DATETIME     NOT NULL,
            INDEX idx_link_images_state (state)
        ) DEFAULT CHARSET=utf8mb4",
    )
    .execute(pool)
    .await
    .context("creating link_images table")?;
    Ok(())
}
