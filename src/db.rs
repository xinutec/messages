//! MariaDB pool. This app reads the shared `signal` database, whose tables the
//! archiver's migrations and import_gchat.py own. It creates only its own
//! `sessions`, `link_images` and `transcripts`, here on boot.

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

    // A link somebody posted, keyed by the URL's hash: `offered`, or `ok` once
    // the bytes are on the volume. A link enters as `offered` when a page
    // containing it is served, with the URL taken from the message, so a request
    // only ever names a hash. A "no" is not a row.
    sqlx::query(
        r"CREATE TABLE IF NOT EXISTS link_images (
            url_hash     CHAR(64)     NOT NULL PRIMARY KEY,
            url          TEXT         NOT NULL,
            state        ENUM('offered','ok') NOT NULL,
            content_type VARCHAR(128) NULL,
            size_bytes   BIGINT       NULL,
            stored_name  VARCHAR(80)  NULL,
            wanted_at    DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP,
            fetched_at   DATETIME     NULL,
            INDEX idx_link_images_queue (state, wanted_at)
        ) DEFAULT CHARSET=utf8mb4",
    )
    .execute(pool)
    .await
    .context("creating link_images table")?;

    // What recall's transcriber heard in an audio attachment.
    // The id is the job id its runner leases. `heard` is the shim's reply, NULL
    // until done and after a refusal (`error`); the words shown are read from it.
    sqlx::query(
        r"CREATE TABLE IF NOT EXISTS transcripts (
            id            BIGINT       NOT NULL AUTO_INCREMENT PRIMARY KEY,
            origin        VARCHAR(16)  NOT NULL,
            attachment_id VARCHAR(255) NOT NULL,
            leased_at     DATETIME     NULL,
            leases        INT          NOT NULL DEFAULT 0,
            done_at       DATETIME     NULL,
            language      VARCHAR(16)  NULL,
            error         VARCHAR(255) NULL,
            heard         LONGTEXT     NULL,
            UNIQUE KEY uniq_transcript (origin, attachment_id)
        ) DEFAULT CHARSET=utf8mb4",
    )
    .execute(pool)
    .await
    .context("creating transcripts table")?;
    // Its first shape stored words, and had neither `heard` nor `leases`; this
    // brings it here.
    sqlx::query(
        "ALTER TABLE transcripts ADD COLUMN IF NOT EXISTS heard LONGTEXT NULL,
           ADD COLUMN IF NOT EXISTS leases INT NOT NULL DEFAULT 0, DROP COLUMN IF EXISTS text",
    )
    .execute(pool)
    .await
    .context("bringing transcripts up to date")?;

    // An earlier shape wrote "not a picture" and "could not reach it" down, with
    // a note and the reader that decided; those links are offered again, and
    // the table brought to the shape above. A no-op once done.
    sqlx::query(
        "UPDATE link_images SET state = 'offered', fetched_at = NULL
          WHERE state NOT IN ('offered', 'ok')",
    )
    .execute(pool)
    .await
    .context("offering again what an earlier version refused")?;
    sqlx::query(
        "ALTER TABLE link_images MODIFY COLUMN state ENUM('offered','ok') NOT NULL,
           DROP COLUMN IF EXISTS note, DROP COLUMN IF EXISTS decided_by",
    )
    .execute(pool)
    .await
    .context("bringing link_images.state up to date")?;

    Ok(())
}
