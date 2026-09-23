//! `/healthz/deep` — whether the ARCHIVE answers, not whether this process does.
//!
//! The 503 needs no database: a pool pointed at nothing is the state this
//! reports, reachable anywhere. The 200 is in `api_routes.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use messages::config::Config;
use messages::state::AppState;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
use tower::ServiceExt;

/// A pool that will never connect: lazy, so building it cannot fail, and
/// pointed at a port nothing listens on.
fn unreachable_archive() -> sqlx::MySqlPool {
    MySqlPoolOptions::new()
        .max_connections(1)
        // Not the probe's whole budget on retries.
        .acquire_timeout(std::time::Duration::from_millis(500))
        .connect_lazy_with(
            MySqlConnectOptions::new()
                .host("127.0.0.1")
                .port(1)
                .username("nobody")
                .database("nothing"),
        )
}

fn cfg() -> Config {
    Config {
        db_options: MySqlConnectOptions::new(),
        session_secret: "test".to_string(),
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

async fn get(uri: &str) -> StatusCode {
    let app = messages::routes::router(AppState::new(
        unreachable_archive(),
        cfg(),
        reqwest::Client::new(),
        None,
    ));
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    app.oneshot(req).await.unwrap().status()
}

/// 503: the front door counts any 5xx as not serving.
#[tokio::test]
async fn an_unreachable_archive_is_a_server_error() {
    assert_eq!(get("/healthz/deep").await, StatusCode::SERVICE_UNAVAILABLE);
}

/// Liveness stays 200 with the same dead pool, or a database blip would become a
/// crashloop.
#[tokio::test]
async fn the_liveness_path_stays_up_when_the_archive_is_down() {
    assert_eq!(get("/healthz").await, StatusCode::OK);
}

/// Unauthenticated: the front door probes without a session.
#[tokio::test]
async fn it_answers_without_a_session() {
    for path in ["/healthz", "/healthz/deep"] {
        assert_ne!(
            get(path).await,
            StatusCode::UNAUTHORIZED,
            "{path} must be probeable with no cookie"
        );
    }
}
