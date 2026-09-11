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

    // What is known about a link somebody posted, keyed by the link itself: the
    // same picture posted in three channels is one row and one fetch.
    //
    // ⚠ **`wanted` IS THE QUEUE, and that is why there is no second table.** A
    // link enters this table the moment a READER is served a message containing
    // it — reading is what makes a picture worth having — and leaves `wanted`
    // when the fetcher has asked. The states are a lifecycle, not a set of flags:
    //
    //     offered → ok          the bytes are on the volume
    //             → not_image   reached it; not a picture we may inline
    //             → failed      could not reach it, or broke the limits
    //
    // ⚠ **`offered` IS WHAT MAKES THE TAP SAFE.** Serving a page registers each
    // of its links here with the URL TAKEN FROM THE ARCHIVE, and asking for one
    // names that row by its hash. The browser therefore never names an address to
    // fetch — if it could, this would be an endpoint that fetches anything anyone
    // asks for, wearing a button. Nothing is fetched at `offered`; a person has
    // to ask, and the answer comes back on that same request.
    //
    // ⚠ **THE REFUSALS ARE ROWS TOO.** A link that is not a picture must be
    // remembered as not one, or every reading of that conversation asks a
    // stranger's server about it again. Somebody else's server hears from us once
    // per link, ever — and only because someone actually read the line.
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

    // The table above predates `wanted` by a day. Both statements are idempotent
    // and cost nothing on a table that already matches; stating them is what
    // makes a running instance reach the shape above without a migration
    // framework this app deliberately does not have.
    for alter in [
        "ALTER TABLE link_images MODIFY COLUMN state ENUM('offered','wanted','ok','not_image','failed') NOT NULL",
        // `wanted` was the queue a CronJob drained. The fetch is synchronous now,
        // so no row rests there; the value stays in the enum only long enough for
        // any straggler to be re-offered, and is not written by anything.
        "ALTER TABLE link_images ADD COLUMN IF NOT EXISTS wanted_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP",
        "ALTER TABLE link_images MODIFY COLUMN fetched_at DATETIME NULL",
        // Which reader decided. NULL means "before this column existed", which is
        // older than any reader and so re-offered — see `link_image::READER_VERSION`.
        "ALTER TABLE link_images ADD COLUMN IF NOT EXISTS decided_by INT NULL",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(alter))
            .execute(pool)
            .await
            .with_context(|| format!("bringing link_images up to date: {alter}"))?;
    }

    // The speculative backfill's watermarks. Dropped rather than left behind: the
    // crawl it paced is gone, and a table nobody writes is a thing the next
    // reader has to work out the meaning of.
    sqlx::query("DROP TABLE IF EXISTS link_fetch_progress")
        .execute(pool)
        .await
        .context("dropping link_fetch_progress")?;

    Ok(())
}
