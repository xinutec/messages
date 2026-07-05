//! Runtime configuration from the environment.
//!
//! The DB connection is assembled from parts (DB_HOST/…), matching the signal
//! ingester's convention, so this app can read the very same `signal-secret`
//! (DB_USER/DB_PASSWORD) in-namespace rather than duplicating a DSN.

use anyhow::{Context, Result};
use sqlx::mysql::MySqlConnectOptions;

#[derive(Clone, Debug)]
pub struct Config {
    /// MariaDB connection options, built from DB_* parts. Built from parts (not a
    /// formatted `mysql://` URL) so a password from the externally-owned
    /// `signal-secret` can contain URL-reserved characters (`@ : / # ?`) without
    /// corrupting the DSN.
    pub db_options: MySqlConnectOptions,
    /// HMAC key for signing session cookies.
    pub session_secret: String,
    /// Address to bind the HTTP server to.
    pub bind_addr: String,

    /// Base URL of the Nextcloud instance, no trailing slash.
    pub nc_base_url: String,
    /// OAuth2 client registered in NC admin (identity flow).
    pub nc_client_id: String,
    pub nc_client_secret: String,
    /// Must match the redirect URI registered for the OAuth2 client.
    pub nc_redirect_uri: String,

    /// Nextcloud user ids permitted to log in. The archive holds private
    /// messages and the host is on a shared VPN, so access is fail-closed: an
    /// empty list (or a user not on it) is rejected. Set via ALLOWED_USERS
    /// (comma-separated).
    pub allowed_users: Vec<String>,

    /// Directory of the built Angular bundle to serve (SPA fallback). Unset →
    /// API-only (dev, where `ng serve` proxies).
    pub static_dir: Option<String>,

    /// Mount of the signal-attachments PVC (read-only); files referenced by
    /// `attachments.stored_path` are served from here by basename.
    pub attachments_dir: String,
}

fn env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("missing required env var {key}"))
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let db_host = env("DB_HOST")?;
        let db_port: u16 = env_or("DB_PORT", "3306")
            .parse()
            .context("DB_PORT must be a port number")?;
        let db_name = env("DB_NAME")?;
        let db_user = env("DB_USER")?;
        let db_password = env("DB_PASSWORD")?;
        let db_options = MySqlConnectOptions::new()
            .host(&db_host)
            .port(db_port)
            .username(&db_user)
            .password(&db_password)
            .database(&db_name);

        let allowed_users = env("ALLOWED_USERS")?
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();

        Ok(Self {
            db_options,
            session_secret: env("SESSION_SECRET")?,
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8080"),
            nc_base_url: env("NC_BASE_URL")?.trim_end_matches('/').to_string(),
            nc_client_id: env("NC_CLIENT_ID")?,
            nc_client_secret: env("NC_CLIENT_SECRET")?,
            nc_redirect_uri: env("NC_REDIRECT_URI")?,
            allowed_users,
            static_dir: std::env::var("STATIC_DIR").ok(),
            attachments_dir: env_or("ATTACHMENTS_DIR", "/attachments"),
        })
    }

    /// Whether a Nextcloud user id is permitted to use the app.
    pub fn is_allowed(&self, user_id: &str) -> bool {
        self.allowed_users.iter().any(|u| u == user_id)
    }
}
