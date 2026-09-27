//! MariaDB archive store: the connection, and the migrations every binary applies
//! at startup; one module of writes per origin.
//!
//! A statement over a list is built with one `?` per value and every value
//! bound; that fixed shape is what each `AssertSqlSafe` asserts.

use anyhow::{Context, Result};
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};

mod irc;
mod migrations;
mod signal;
mod telegram;

pub use irc::IrcConversations;
pub use migrations::{
    BACKFILL_LINK_PREVIEWS, BACKFILL_QUOTES, BACKFILL_SERVER_TIMES, BACKFILL_TEXT_STYLES,
};
pub use telegram::{
    TelegramBackfill, TelegramDeleteScope, TelegramMediaState, TelegramReadDirection,
    TelegramStored,
};

use migrations::MIGRATIONS;

/// The MariaDB DSN, from the `DB_*` environment every binary shares.
///
/// The password is not escaped, so one containing `@` or `/` would break the DSN.
pub fn url_from_env() -> Result<String> {
    let host = std::env::var("DB_HOST").context("DB_HOST not set")?;
    let port = std::env::var("DB_PORT").unwrap_or_else(|_| "3306".to_string());
    let name = std::env::var("DB_NAME").context("DB_NAME not set")?;
    let user = std::env::var("DB_USER").context("DB_USER not set")?;
    let pass = std::env::var("DB_PASSWORD").context("DB_PASSWORD not set")?;
    Ok(format!("mysql://{user}:{pass}@{host}:{port}/{name}"))
}

/// Rows per statement, well inside MySQL's 65,535-placeholder cap.
const INSERT_CHUNK: usize = 1_000;

#[derive(Clone)]
pub struct Db {
    pool: MySqlPool,
}

impl Db {
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = MySqlPoolOptions::new()
            .max_connections(5)
            .connect(url)
            .await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    /// Apply outstanding migrations, one process at a time: every binary runs
    /// this at startup, and a deploy starts them together.
    ///
    /// The advisory lock belongs to a connection, so the lock, the migrations
    /// and the release all run on one.
    async fn migrate(&self) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS schema_version (version INT PRIMARY KEY)")
            .execute(&mut *conn)
            .await?;
        // Waits until it holds the lock; 1 is granted, 0 a timeout.
        loop {
            let granted: Option<i64> = sqlx::query_scalar("SELECT GET_LOCK('signal_migrate', 30)")
                .fetch_one(&mut *conn)
                .await?;
            if granted == Some(1) {
                break;
            }
            tracing::info!("waiting for another process to finish migrating");
        }
        let applied: Vec<i32> = sqlx::query_scalar("SELECT version FROM schema_version")
            .fetch_all(&mut *conn)
            .await?;
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let v = i as i32;
            if !applied.contains(&v) {
                tracing::info!("applying migration v{v}");
                sqlx::query(*sql).execute(&mut *conn).await?;
                sqlx::query("INSERT INTO schema_version (version) VALUES (?)")
                    .bind(v)
                    .execute(&mut *conn)
                    .await?;
            }
        }
        sqlx::query("SELECT RELEASE_LOCK('signal_migrate')")
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    /// The pool, for the Telegram session store, which implements a `grammers` trait.
    pub fn pool(&self) -> &MySqlPool {
        &self.pool
    }
}
