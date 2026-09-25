//! A missing file must 404, not be handed the page as a 200. A dot in the last
//! path segment marks a file: `/c/irc/7` is a route, `/main-ABC123.js` a file.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use messages::config::Config;
use messages::routes;
use messages::state::AppState;
use tower::ServiceExt;

#[path = "support/config.rs"]
mod support;

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
        static_dir: Some(dir.0.to_string_lossy().into_owned()),
        ..support::config()
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

/// A client-side route still loads the shell.
#[tokio::test]
async fn a_deep_link_still_gets_the_page() {
    let (status, ct) = get("/c/irc/7").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        ct.starts_with("text/html"),
        "a route did not get the page: {ct}"
    );
}

/// A file that exists is served as itself.
#[tokio::test]
async fn a_real_asset_is_still_served() {
    let (status, ct) = get("/main-ABC123.js").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !ct.starts_with("text/html"),
        "the bundle came back as HTML: {ct}"
    );
}
