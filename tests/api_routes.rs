//! The API surface, through the real router.
//!
//! Everything else tests the DECISIONS the handlers make; this tests that a
//! request actually reaches them, and that the wiring in `routes::mod` puts the
//! auth extractor in front of every route that needs one. `routes/api.rs` had no
//! test exercising it through an HTTP request until 2026-09-03.
//!
//! ⚠ **These deliberately touch no archive table.** `tests/archive.rs` DROPs and
//! recreates them, and cargo runs test binaries in parallel against the one
//! database, so a fixture here would race its DDL. Every case below is decided
//! before the handler reaches the archive — an unknown origin, a non-IRC send, an
//! empty search, a missing or forged cookie — which is the auth-and-routing
//! surface and exactly what was untested. The one guard left uncovered for this
//! reason is `is_status`, which needs `irc_conversations`.
//!
//! Skipped without `MESSAGES_TEST_DATABASE_URL`, like the other DB tests. The
//! only table used is `sessions`, which this app owns.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use messages::config::Config;
use messages::session::{self, UserSession};
use messages::state::AppState;
use sqlx::MySqlPool;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
use tower::ServiceExt;

const SECRET: &str = "test session secret";

async fn pool() -> Option<MySqlPool> {
    let url = std::env::var("MESSAGES_TEST_DATABASE_URL").ok()?;
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

fn cfg() -> Config {
    Config {
        db_options: MySqlConnectOptions::new(),
        session_secret: SECRET.to_string(),
        bind_addr: String::new(),
        nc_base_url: "https://nc.invalid".to_string(),
        nc_client_id: String::new(),
        nc_client_secret: String::new(),
        nc_redirect_uri: String::new(),
        allowed_users: vec!["pippijn".to_string()],
        static_dir: None,
        attachments_dir: "/nonexistent".to_string(),
        // No send key: the archive still serves and every send is refused. The
        // send cases below are all decided before this is consulted.
        irc_send: None,
    }
}

fn app(pool: MySqlPool) -> axum::Router {
    messages::routes::router(AppState::new(pool, cfg(), reqwest::Client::new(), None))
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

/// ⚠ The whole API is private. A route added without the extractor would serve
/// the archive to anyone who asked, and nothing but this would say so.
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
    // A plausible-looking id with a signature this secret never produced.
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

/// An unknown origin is a 404, not a 400: the URL names a conversation that does
/// not exist. Signed in, so this cannot pass for the 401 reason.
#[tokio::test]
async fn an_unknown_origin_is_not_found() {
    let Some(pool) = pool().await else { return };
    let cookie = signed_in(&pool).await;
    assert_eq!(
        go(
            &pool,
            "GET",
            "/api/conversations/telegram/7/messages",
            Some(&cookie)
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

/// ⚠ **Signal cannot be sent to, and that is a DECISION rather than a gap.**
/// `signal-cli-rest-api` is a linked device in the same namespace and could
/// send; what stops it is that an IRC echo is confirmable against irssi's log
/// where a Signal echo would be the only evidence the message existed. A future
/// change that "adds Signal support" by widening this check should have to
/// delete this test and read why.
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

/// Health is deliberately outside the gate — a probe has no cookie.
#[tokio::test]
async fn healthz_needs_no_session() {
    let Some(pool) = pool().await else { return };
    assert_eq!(go(&pool, "GET", "/healthz", None).await, StatusCode::OK);
}
