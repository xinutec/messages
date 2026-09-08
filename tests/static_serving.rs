//! **A missing FILE must 404, not be handed the page.**
//!
//! #1478, measured across the fleet 2026-09-08: `GET /media/nope.woff2` came
//! back `200 text/html` — the SPA shell, to a browser that asked for a font. It
//! renders broken icons and reports nothing at all, so the failure is silent on
//! both sides, and the wrong answer being a `200` is exactly what makes it
//! invisible.
//!
//! ⚠ This app was NOT in the task's original list. That list came from probing
//! two names; asking every host in `frontdoor.json` found eight, this one among
//! them — the same defect #881 was reopened for, one week later.
//!
//! The rule is a dot in the last path segment: `/c/irc/7` is a route and
//! `/main-ABC123.js` is a file. A heuristic, and the alternative — enumerating
//! the bundle's own asset names — would have to be rebuilt whenever `ng build`
//! changes a hash.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use messages::config::Config;
use messages::routes;
use messages::state::AppState;
use sqlx::mysql::MySqlConnectOptions;
use tower::ServiceExt;

/// A static dir shaped like a real `ng build` output.
struct StaticDir(std::path::PathBuf);

impl StaticDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "messages-serving-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("create static dir");
        std::fs::write(dir.join("index.html"), "<!doctype html><html></html>").expect("index");
        std::fs::write(dir.join("main-ABC123.js"), "export {};").expect("bundle");
        Self(dir)
    }
}

impl Drop for StaticDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn get(path: &str) -> (StatusCode, String) {
    let dir = StaticDir::new();
    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .connect_lazy("mysql://unused:unused@127.0.0.1:1/unused")
        .expect("lazy pool");
    let cfg = Config {
        db_options: MySqlConnectOptions::new(),
        session_secret: String::new(),
        bind_addr: String::new(),
        nc_base_url: String::new(),
        nc_client_id: String::new(),
        nc_client_secret: String::new(),
        nc_redirect_uri: String::new(),
        allowed_users: Vec::new(),
        static_dir: Some(dir.0.to_string_lossy().into_owned()),
        attachments_dir: String::new(),
        irc_send: None,
    };
    let res = routes::router(AppState::new(pool, cfg, reqwest::Client::new(), None))
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    (status, ct)
}

#[tokio::test]
async fn a_missing_asset_is_a_404_and_not_the_page() {
    let (status, ct) = get("/media/nope.woff2").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        !ct.starts_with("text/html"),
        "a font request got HTML: {ct}"
    );
}

/// The other half, and the one a careless fix breaks: a client-side route has
/// no dot and must still load the shell, or every deep link 404s.
#[tokio::test]
async fn a_deep_link_still_gets_the_page() {
    let (status, ct) = get("/c/irc/7").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        ct.starts_with("text/html"),
        "a route did not get the page: {ct}"
    );
}

/// And a file that EXISTS is still served as itself.
#[tokio::test]
async fn a_real_asset_is_still_served() {
    let (status, ct) = get("/main-ABC123.js").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !ct.starts_with("text/html"),
        "the bundle came back as HTML: {ct}"
    );
}
