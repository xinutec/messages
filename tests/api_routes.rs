//! The API surface, through the real router.
//!
//! That a request reaches the handlers, and that `routes::mod` puts the auth
//! extractor in front of every route that needs one.
//!
//! No archive table is touched: `tests/archive.rs` drops and recreates them in
//! the same database, in parallel. Every case is decided before the handler
//! reads the archive; the `is_status` guard is tested in `tests/archive.rs`.
//! Only `sessions` is used. Skipped without `MESSAGES_TEST_DATABASE_URL`, except
//! in CI.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use messages::session::{self, UserSession};
use messages::state::AppState;
use sqlx::MySqlPool;
use sqlx::mysql::MySqlPoolOptions;
use tower::ServiceExt;

#[path = "support/database.rs"]
mod database;
#[path = "support/config.rs"]
mod support;

/// The secret `support::config` signs with.
const SECRET: &str = "test session secret";

async fn pool() -> Option<MySqlPool> {
    let url = database::database_url()?;
    let pool = MySqlPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect to MESSAGES_TEST_DATABASE_URL");
    messages::db::ensure_schema(&pool)
        .await
        .expect("sessions table");
    Some(pool)
}

fn app(pool: MySqlPool) -> axum::Router {
    messages::routes::router(AppState::new(
        pool,
        support::config(),
        reqwest::Client::new(),
        None,
    ))
}

async fn go(pool: &MySqlPool, method: &str, uri: &str, cookie: Option<&str>) -> StatusCode {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        req = req.header("cookie", format!("{}={c}", session::COOKIE_NAME));
    }
    let body = if method == "POST" {
        Body::from(r#"{"text":"hello"}"#)
    } else {
        Body::empty()
    };
    let req = req
        .header("content-type", "application/json")
        .body(body)
        .unwrap();
    app(pool.clone()).oneshot(req).await.unwrap().status()
}

async fn signed_in(pool: &MySqlPool) -> String {
    session::create_session(
        pool,
        SECRET,
        &UserSession {
            user_id: "pippijn".to_string(),
            display_name: "Pippijn".to_string(),
        },
    )
    .await
    .expect("create session")
}

/// The 200 half of `/healthz/deep`, which needs a database;
/// `tests/deep_health.rs` has the 503.
#[tokio::test]
async fn a_reachable_archive_reports_itself_healthy() {
    let Some(pool) = pool().await else { return };
    assert_eq!(
        go(&pool, "GET", "/healthz/deep", None).await,
        StatusCode::OK
    );
}

/// Every API route requires a session.
#[tokio::test]
async fn every_api_route_refuses_a_request_with_no_cookie() {
    let Some(pool) = pool().await else { return };
    for (method, uri) in [
        ("GET", "/api/me"),
        ("GET", "/api/conversations"),
        ("GET", "/api/conversations/irc/7/messages"),
        ("GET", "/api/search?q=hello"),
        ("GET", "/api/attachments/1"),
        ("POST", "/api/conversations/irc/7/send"),
    ] {
        assert_eq!(
            go(&pool, method, uri, None).await,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} did not require a session"
        );
    }
}

#[tokio::test]
async fn a_forged_cookie_is_refused_like_no_cookie_at_all() {
    let Some(pool) = pool().await else { return };
    // A plausible id with a signature this secret never produced.
    let forged = format!("{}.{}", "a".repeat(64), "0".repeat(64));
    assert_eq!(
        go(&pool, "GET", "/api/me", Some(&forged)).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_real_session_gets_in() {
    let Some(pool) = pool().await else { return };
    let cookie = signed_in(&pool).await;
    assert_eq!(
        go(&pool, "GET", "/api/me", Some(&cookie)).await,
        StatusCode::OK
    );
}

/// An unknown origin is a 404. Signed in, so it cannot pass as a 401. The name
/// must never become a real origin.
#[tokio::test]
async fn an_unknown_origin_is_not_found() {
    let Some(pool) = pool().await else { return };
    let cookie = signed_in(&pool).await;
    assert_eq!(
        go(
            &pool,
            "GET",
            "/api/conversations/carrier-pigeon/7/messages",
            Some(&cookie)
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

/// Only IRC can be sent to. Signal is read-only by decision (see `api::send`);
/// Telegram's echo would be confirmable, but sending there is not designed yet.
/// These are real origins: the claim is that a known non-IRC origin is refused.
#[tokio::test]
async fn sending_is_refused_for_every_origin_but_irc() {
    let Some(pool) = pool().await else { return };
    let cookie = signed_in(&pool).await;
    for origin in ["signal", "gchat", "telegram"] {
        assert_eq!(
            go(
                &pool,
                "POST",
                &format!("/api/conversations/{origin}/x/send"),
                Some(&cookie)
            )
            .await,
            StatusCode::NOT_FOUND,
            "{origin} was allowed to send"
        );
    }
}

/// Health needs no session; a probe has no cookie.
#[tokio::test]
async fn healthz_needs_no_session() {
    let Some(pool) = pool().await else { return };
    assert_eq!(go(&pool, "GET", "/healthz", None).await, StatusCode::OK);
}
