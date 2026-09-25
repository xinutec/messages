//! A `Config` for tests: nothing it names is reached, and a test overrides the
//! fields it exercises with struct update syntax.

use messages::config::Config;
use sqlx::mysql::MySqlConnectOptions;

pub fn config() -> Config {
    Config {
        db_options: MySqlConnectOptions::new(),
        session_secret: "test session secret".to_string(),
        bind_addr: String::new(),
        nc_base_url: "https://nc.invalid".to_string(),
        nc_client_id: String::new(),
        nc_client_secret: String::new(),
        nc_redirect_uri: String::new(),
        allowed_users: vec!["pippijn".to_string()],
        static_dir: None,
        attachments_dir: "/nonexistent".to_string(),
        link_images_dir: "/link-images".into(),
        telegram_media_dir: "/telegram-media".into(),
        link_fetcher_url: "http://link-fetch.invalid".into(),
        irc_send: None,
    }
}
