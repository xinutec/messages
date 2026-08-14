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

    /// Where irssi is, for the one thing this app does that is not a read.
    /// `None` disables sending entirely — see [`IrcSend`].
    pub irc_send: Option<IrcSend>,
}

/// How to reach the irssi that holds Pippijn's IRC connections.
///
/// ⚠ **Optional on purpose, and that is a safety property rather than a
/// convenience.** This app is a reader everywhere else; sending is the one
/// capability that acts as a person on networks other people are on. If the key
/// is not mounted the app still boots and still serves the archive — it just
/// refuses to send. The alternative, failing to start, would take a working
/// viewer down over a capability it does not need in order to read.
#[derive(Clone, Debug)]
pub struct IrcSend {
    /// amun over WireGuard. An address rather than a name: this is a different
    /// cluster and nothing here resolves its names.
    pub host: String,
    pub port: u16,
    /// The mounted secret: `id_ed25519` and `known_hosts`.
    pub key_dir: String,
    /// Writable scratch. ssh refuses a private key carrying any group or other
    /// bit, and a secret volume cannot present one that this pod can read and
    /// ssh will accept — so the key is copied here at 0400 before use.
    pub work_dir: String,
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
            irc_send: Self::irc_send_from_env()?,
        })
    }

    /// All four settings or none. A partially configured send path would fail at
    /// the first send rather than at boot, which is the wrong end to find out.
    fn irc_send_from_env() -> Result<Option<IrcSend>> {
        let Ok(host) = std::env::var("IRC_SEND_HOST") else {
            return Ok(None);
        };
        Ok(Some(IrcSend {
            host,
            port: env("IRC_SEND_PORT")?
                .parse()
                .context("IRC_SEND_PORT must be a port number")?,
            key_dir: env("IRC_SEND_KEY_DIR")?,
            work_dir: env("IRC_SEND_WORK_DIR")?,
        }))
    }

    /// Whether a Nextcloud user id is permitted to use the app.
    pub fn is_allowed(&self, user_id: &str) -> bool {
        self.allowed_users.iter().any(|u| u == user_id)
    }
}
