//! Runtime configuration from the environment.
//!
//! The database connection is built from DB_* parts, as the signal ingester's
//! is, so this app reads the same `signal-secret`.

use anyhow::{Context, Result};
use sqlx::mysql::MySqlConnectOptions;

/// Split `ALLOWED_USERS` into the ids that may use the app.
///
/// Empty entries are dropped: an empty id would match a caller presenting an
/// empty user id. Every way of configuring nothing admits nobody. Separate from
/// `from_env` so tests/access.rs can call it without touching the environment.
pub fn parse_allowed_users(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[derive(Clone, Debug)]
pub struct Config {
    /// MariaDB options, built from DB_* parts so a password with URL-reserved
    /// characters is safe.
    pub db_options: MySqlConnectOptions,
    /// HMAC key for signing session cookies.
    pub session_secret: String,
    /// Address to bind the HTTP server to.
    pub bind_addr: String,

    /// Base URL of the Nextcloud instance, no trailing slash. Checked to parse
    /// at load.
    pub nc_base_url: String,
    /// OAuth2 client registered in NC admin (identity flow).
    pub nc_client_id: String,
    pub nc_client_secret: String,
    /// Must match the redirect URI registered for the OAuth2 client.
    pub nc_redirect_uri: String,

    /// Nextcloud user ids permitted to log in (`ALLOWED_USERS`, comma-separated).
    /// Fail-closed: an empty list admits nobody.
    pub allowed_users: Vec<String>,

    /// The built Angular bundle to serve. Unset: API-only.
    pub static_dir: Option<String>,

    /// The signal-attachments mount (read-only); files are served by basename.
    pub attachments_dir: String,

    /// The link-images mount (read-only here).
    pub link_images_dir: String,
    /// Where the Telegram feed writes fetched media. Mounted read-only here.
    pub telegram_media_dir: String,

    /// The in-cluster fetch service; see `bin/link-fetch.rs`.
    pub link_fetcher_url: String,

    /// Where irssi is. `None` disables sending; see [`IrcSend`].
    pub irc_send: Option<IrcSend>,
}

/// How to reach the irssi that holds Pippijn's IRC connections.
///
/// Optional: without the key the app still serves the archive and refuses to
/// send.
#[derive(Clone, Debug)]
pub struct IrcSend {
    /// amun over WireGuard, as an address: this cluster cannot resolve its names.
    pub host: String,
    pub port: u16,
    /// The mounted secret: `id_ed25519` and `known_hosts`.
    pub key_dir: String,
    /// Writable scratch, where the key is copied at 0400 for ssh.
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

        let allowed_users = parse_allowed_users(&env("ALLOWED_USERS")?);

        let nc_base_url = env("NC_BASE_URL")?.trim_end_matches('/').to_string();
        url::Url::parse(&nc_base_url).context("NC_BASE_URL must be a URL")?;

        Ok(Self {
            db_options,
            session_secret: env("SESSION_SECRET")?,
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8080"),
            nc_base_url,
            nc_client_id: env("NC_CLIENT_ID")?,
            nc_client_secret: env("NC_CLIENT_SECRET")?,
            nc_redirect_uri: env("NC_REDIRECT_URI")?,
            allowed_users,
            static_dir: std::env::var("STATIC_DIR").ok(),
            attachments_dir: env_or("ATTACHMENTS_DIR", "/attachments"),
            link_images_dir: env_or("LINK_IMAGES_DIR", "/link-images"),
            telegram_media_dir: env_or("TELEGRAM_MEDIA_DIR", "/telegram-media"),
            link_fetcher_url: env_or("LINK_FETCHER_URL", "http://messages-link-fetch:8080"),
            irc_send: Self::irc_send_from_env()?,
        })
    }

    /// All four or none, so a partial configuration fails at boot.
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
