//! `/healthz/deep` — whether the ARCHIVE answers, not whether this process does.
//!
//! The 503 needs no database: a pool pointed at nothing is the state this
//! reports, reachable anywhere. The 200 is in `api_routes.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use messages::state::AppState;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
use tower::ServiceExt;

#[path = "support/config.rs"]
mod support;

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

async fn get(uri: &str) -> StatusCode {
    let app = messages::routes::router(AppState::new(
        unreachable_archive(),
        support::config(),
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
