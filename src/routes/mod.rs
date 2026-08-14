//! HTTP routing table.

pub mod api;
pub mod auth;
pub mod telemetry;

use axum::Router;
use axum::http::{HeaderValue, Response, header};
use axum::routing::{get, post};
use tower::ServiceBuilder;
use tower_http::services::fs::ServeFileSystemResponseBody;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::state::AppState;

/// How long a static response may be reused without asking again.
///
/// ⚠ **`index.html` MUST REVALIDATE, and shipping it without saying so cost a
/// deploy that nobody could see.** With no `Cache-Control` at all — which is
/// what this served until 2026-08-14 — a client falls back to *heuristic*
/// caching from `Last-Modified`, and is free to keep the document for as long as
/// it likes without ever asking. An Android WebView did exactly that: it fetched
/// `/api/me`, `/api/conversations` and a whole thread, and never once requested
/// `main-*.js`. The app on the phone was several builds old while the server had
/// been serving the new one for hours, and the only visible symptom was a
/// missing button.
///
/// `no-cache` rather than `no-store`: it means "ask first", not "never keep", so
/// the ETag still turns the usual case into a 304 with no body.
///
/// Everything else Angular emits carries a content hash in its NAME, so a new
/// build is a new URL and the old one can never be wrong. Those are the one kind
/// of response `immutable` is honestly available for.
fn cache_control_for(res: &Response<ServeFileSystemResponseBody>) -> Option<HeaderValue> {
    let is_html = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/html"));
    Some(if is_html {
        HeaderValue::from_static("no-cache")
    } else {
        HeaderValue::from_static("public, max-age=31536000, immutable")
    })
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/me", get(api::me))
        .route("/conversations", get(api::conversations))
        .route("/conversations/{origin}/{id}/messages", get(api::messages))
        // The app's only write, and the only route another person can observe
        // the effect of. IRC only; see `api::send`.
        .route("/conversations/{origin}/{id}/send", post(api::send))
        .route("/attachments/{id}", get(api::attachment))
        .route("/search", get(api::search))
        // What the person did, folded into the same log as what the API saw.
        .route("/telemetry", post(telemetry::record));

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/login", get(auth::login))
        .route("/auth/callback", get(auth::callback))
        .route("/logout", post(auth::logout))
        .nest("/api", api);

    // Serve the built Angular bundle (single origin), SPA-fallback to index.html.
    // API-only when STATIC_DIR is unset (dev: `ng serve` proxies).
    let app = if let Some(dir) = state.cfg.static_dir.clone() {
        let serve = ServeDir::new(&dir).fallback(ServeFile::new(format!("{dir}/index.html")));
        // The layer wraps only the static service: an API response is neither
        // a document to revalidate nor an immutable asset, and giving JSON a
        // year-long `immutable` would be the same bug pointing the other way.
        let serve = ServiceBuilder::new()
            .layer(SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                cache_control_for,
            ))
            .service(serve);
        app.fallback_service(serve)
    } else {
        app
    };

    app.with_state(state)
}
