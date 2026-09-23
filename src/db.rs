//! MariaDB pool. This app reads the shared `signal` database, whose tables the
//! signal repo's migrations and import_gchat.py own. It creates only its own
//! `sessions` and `link_images`, here on boot.

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

    // What is known about a link somebody posted, keyed by the URL's hash.
    //
    //     offered → ok          the bytes are on the volume
    //             → not_image   reached it; not a picture we may inline
    //             → failed      could not reach it, or broke the limits
    //
    // A link enters as `offered` when a page containing it is served, with the
    // URL taken from the message, so a request only ever names a hash. Refusals
    // are rows too, so a server is not asked again on every read.
    sqlx::query(
        r"CREATE TABLE IF NOT EXISTS link_images (
            url_hash     CHAR(64)     NOT NULL PRIMARY KEY,
            url          TEXT         NOT NULL,
            state        ENUM('offered','ok','not_image','failed') NOT NULL,
            content_type VARCHAR(128) NULL,
            size_bytes   BIGINT       NULL,
            stored_name  VARCHAR(80)  NULL,
            note         VARCHAR(255) NULL,
            wanted_at    DATETIME     NOT NULL,
            fetched_at   DATETIME     NULL,
            INDEX idx_link_images_queue (state, wanted_at)
        ) DEFAULT CHARSET=utf8mb4",
    )
    .execute(pool)
    .await
    .context("creating link_images table")?;

    // Brings an existing table to the shape above; idempotent.
    for alter in [
        "ALTER TABLE link_images MODIFY COLUMN state ENUM('offered','wanted','ok','not_image','failed') NOT NULL",
        // `wanted` is no longer written; it stays in the enum for old rows.
        "ALTER TABLE link_images ADD COLUMN IF NOT EXISTS wanted_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP",
        "ALTER TABLE link_images MODIFY COLUMN fetched_at DATETIME NULL",
        // NULL predates the column, older than any reader, so re-offered; see
        // `link_image::READER_VERSION`.
        "ALTER TABLE link_images ADD COLUMN IF NOT EXISTS decided_by INT NULL",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(alter))
            .execute(pool)
            .await
            .with_context(|| format!("bringing link_images up to date: {alter}"))?;
    }

    // A table the removed link backfill used.
    sqlx::query("DROP TABLE IF EXISTS link_fetch_progress")
        .execute(pool)
        .await
        .context("dropping link_fetch_progress")?;

    Ok(())
}
