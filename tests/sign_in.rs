//! A whole sign-in through the real router, against a stand-in Nextcloud that
//! grants any code: the cookies it leaves behind.
//!
//! Skipped without `MESSAGES_TEST_DATABASE_URL`, except in CI: the session is a
//! row.

use axum::Json;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::routing::{get, post};
use messages::config::Config;
use messages::state::AppState;
use sqlx::MySqlPool;
use sqlx::mysql::MySqlPoolOptions;
use tower::ServiceExt;

#[path = "support/database.rs"]
mod database;
#[path = "support/config.rs"]
mod support;

async fn pool() -> Option<MySqlPool> {
    let url = database::database_url()?;
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to MESSAGES_TEST_DATABASE_URL");
    messages::db::ensure_schema(&pool)
        .await
        .expect("sessions table");
    Some(pool)
}

/// A Nextcloud that trades any code for a token naming `user`.
async fn nextcloud() -> String {
    let nc = axum::Router::new()
        .route(
            "/index.php/apps/oauth2/api/v1/token",
            post(|| async { Json(serde_json::json!({ "access_token": "token" })) }),
        )
        .route(
            "/ocs/v2.php/cloud/user",
            get(|| async {
                Json(serde_json::json!({ "ocs": { "data": { "id": "user", "displayname": "User" } } }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, nc).await.unwrap() });
    format!("http://{addr}")
}

async fn send(app: &axum::Router, req: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(req).await.unwrap()
}

fn set_cookies(res: &axum::response::Response) -> Vec<String> {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

/// The `Set-Cookie` line for `name`, which a test expects to find.
fn cookie_line<'a>(lines: &'a [String], name: &str) -> &'a str {
    lines
        .iter()
        .find(|l| l.starts_with(&format!("{name}=")))
        .unwrap_or_else(|| panic!("no Set-Cookie for {name}: {lines:?}"))
}

/// A cookie set at `Path=/` is removed only by a removal at `Path=/`. Without
/// one, the browser scopes the removal to the request's directory — `/auth`
/// for the callback — and the cookie it meant to drop stays.
#[tokio::test]
async fn signing_in_and_out_removes_each_cookie_where_it_was_set() {
    let Some(pool) = pool().await else { return };
    let cfg = Config {
        nc_base_url: nextcloud().await,
        ..support::config()
    };
    let app = messages::routes::router(AppState::new(pool, cfg, reqwest::Client::new(), None));

    let login = send(&app, Request::get("/login").body(Body::empty()).unwrap()).await;
    let pending = cookie_line(&set_cookies(&login), messages::pending_login::COOKIE_NAME)
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let to_nc = login.headers()[header::LOCATION].to_str().unwrap();
    let state = url::Url::parse(to_nc)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .expect("the authorize URL carries a state");

    let back = send(
        &app,
        Request::get(format!("/auth/callback?code=granted&state={state}"))
            .header(header::COOKIE, &pending)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(back.status(), StatusCode::SEE_OTHER);
    let lines = set_cookies(&back);
    let dropped = cookie_line(&lines, messages::pending_login::COOKIE_NAME);
    assert!(dropped.contains("Max-Age=0"), "{dropped}");
    assert!(
        dropped.contains("Path=/"),
        "removed where it was set: {dropped}"
    );
    let session = cookie_line(&lines, messages::session::COOKIE_NAME)
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    let out = send(
        &app,
        Request::post("/logout")
            .header(header::COOKIE, &session)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(out.status(), StatusCode::NO_CONTENT);
    let lines = set_cookies(&out);
    let dropped = cookie_line(&lines, messages::session::COOKIE_NAME);
    assert!(dropped.contains("Max-Age=0"), "{dropped}");
    assert!(
        dropped.contains("Path=/"),
        "removed where it was set: {dropped}"
    );
}
