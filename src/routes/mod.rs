//! HTTP routing table.

pub mod api;
pub mod auth;
pub mod health;
pub mod telemetry;

use axum::Router;
use axum::http::{HeaderValue, Response, header};
use axum::routing::{get, post};
use tower::ServiceBuilder;
use tower_http::services::ServeDir;
use tower_http::services::fs::ServeFileSystemResponseBody;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::state::AppState;

/// How long a static response may be reused without asking again.
///
/// `index.html` must revalidate: without `Cache-Control` a client caches it
/// heuristically and runs an old build indefinitely. `no-cache` means ask first,
/// so the ETag still makes it a 304. Everything else Angular emits has a content
/// hash in its name, so it is `immutable`.
fn cache_control_for(res: &Response<ServeFileSystemResponseBody>) -> Option<HeaderValue> {
    // Only a response with content says how long it keeps: a 404 marked
    // `immutable` would not be asked for again for a year. 304 counts, since it
    // must carry the headers a 200 would.
    if res.status().is_client_error() || res.status().is_server_error() {
        return None;
    }
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

/// Serve the app's page for a client-side ROUTE, and 404 anything that plainly
/// named a file.
///
/// A missing file must not be answered with the page: that is a 200, and a
/// browser asking for a font gets HTML without complaint. A dot in the last path
/// segment marks a file; heuristic, but needs no list of asset names.
fn spa(index: &str, path: &str) -> axum::response::Response {
    use axum::response::IntoResponse as _;

    if path
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.'))
    {
        return (axum::http::StatusCode::NOT_FOUND, "not found").into_response();
    }
    match std::fs::read_to_string(index) {
        Ok(page) => axum::response::Html(page).into_response(),
        Err(error) => {
            // STATIC_DIR without an index is a misconfigured deployment.
            tracing::error!("the app's index could not be read: {error}");
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "no index").into_response()
        }
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/me", get(api::me))
        .route("/conversations", get(api::conversations))
        .route("/conversations/{origin}/{id}/messages", get(api::messages))
        // The only write; see `api::send`.
        .route("/conversations/{origin}/{id}/send", post(api::send))
        .route("/attachments/{id}", get(api::attachment))
        .route("/gchat-attachments/{id}", get(api::gchat_attachment))
        .route("/telegram-media/{id}", get(api::telegram_media))
        .route("/telegram-media/{id}/state", get(api::telegram_media_state))
        .route(
            "/telegram-media/{id}/request",
            post(api::request_telegram_media),
        )
        .route("/link-images/{id}", get(api::link_image))
        .route("/link-images/{id}/request", post(api::request_link_image))
        .route("/search", get(api::search))
        .route("/telemetry", post(telemetry::record));

    let app = Router::new()
        // Liveness target; must not check dependencies, or a database blip
        // becomes a crashloop. `/api/health` asks whether the archive is readable.
        .route("/healthz", get(|| async { "ok" }))
        .route("/healthz/deep", get(health::deep))
        .route("/login", get(auth::login))
        .route("/auth/callback", get(auth::callback))
        .route("/logout", post(auth::logout))
        .nest("/api", api);

    // The Angular bundle, with SPA fallback to index.html. API-only when
    // STATIC_DIR is unset (`ng serve` proxies in dev).
    let app = if let Some(dir) = state.cfg.static_dir.clone() {
        let index = format!("{dir}/index.html");
        let serve = ServeDir::new(&dir).fallback(get(move |uri: axum::http::Uri| {
            let index = index.clone();
            async move { spa(&index, uri.path()) }
        }));
        // Only the static service: API responses are neither.
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
