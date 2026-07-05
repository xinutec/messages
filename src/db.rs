//! MariaDB pool. This app is a READ-ONLY consumer of the shared `signal`
//! database — the Signal tables are owned by the signal ingester's migrations
//! and the `gchat_*` tables by import_gchat.py. The only table this app owns is
//! its own `sessions`, created here on boot (CREATE TABLE IF NOT EXISTS), kept
//! deliberately out of any cross-app migration framework.

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

/// Create the app's own `sessions` table if absent. Idempotent.
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
    Ok(())
}
