//! HTTP routing table.

pub mod api;
pub mod auth;
pub mod health;
pub mod telemetry;
pub mod transcribe;

use axum::Router;
use axum::http::{HeaderValue, header};
use axum::routing::{get, post, put};
use tower::ServiceBuilder;
use tower_http::services::ServeDir;

use crate::state::AppState;

/// A name carrying its content hash (`main-7GNM4EPX.js`, a font's `-ABCD1234`):
/// a new build is a new name, so it may be cached for ever.
fn is_content_hashed(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or("");
    let Some((stem, _ext)) = name.rsplit_once('.') else {
        return false;
    };
    let Some((_, hash)) = stem.rsplit_once('-') else {
        return false;
    };
    hash.len() == 8
        && hash
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// How long a static response may be reused. A hashed file is `immutable`.
/// Everything else revalidates (`no-cache`: ask first, then a 304 via the ETag):
/// the page names the bundle, and an icon, the manifest and the service worker's
/// files keep their names across builds, so a year of `immutable` pins the old
/// one on every phone. Without `Cache-Control` a WebView caches heuristically and
/// runs a build several deploys old. An error is never `immutable`, or the name is
/// not asked for again; a 304 keeps what a 200 would say.
async fn cache_headers(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let hashed = is_content_hashed(req.uri().path());
    let mut res = next.run(req).await;
    let failed = res.status().is_client_error() || res.status().is_server_error();
    let value = if hashed && !failed {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    res
}

/// Serve the app's page for a client-side ROUTE, and 404 anything that plainly
/// named a file.
///
/// A missing file must not be answered with the page: that is a 200, and a
/// browser asking for a font gets HTML without complaint. A dot in the last path
/// segment marks a file; heuristic, but needs no list of asset names.
async fn spa(index: &str, path: &str) -> axum::response::Response {
    use axum::response::IntoResponse as _;

    if path
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.'))
    {
        return (axum::http::StatusCode::NOT_FOUND, "not found").into_response();
    }
    match tokio::fs::read_to_string(index).await {
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
        .route("/conversations/{origin}/{id}/avatar", get(api::avatar))
        .route("/conversations/{origin}/{id}/send", post(api::send))
        .route("/attachments/{id}", get(api::attachment))
        .route("/link-previews/{id}/image", get(api::link_preview_image))
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
        // becomes a crashloop. `/healthz/deep` asks whether the archive is readable.
        .route("/healthz", get(|| async { "ok" }))
        // dev-lint: allow-unauthenticated-route a probe: answers only whether the archive is readable
        .route("/healthz/deep", get(health::deep))
        .route("/login", get(auth::login))
        .route("/auth/callback", get(auth::callback))
        .route("/logout", post(auth::logout))
        .nest("/api", api);

    // recall's transcriber, at the paths its runner uses. Every one answers 404
    // while no token is configured, as if absent.
    let app = app
        .route("/work/v1/lease", put(transcribe::lease))
        .route("/work/v1/jobs/{id}/done", put(transcribe::done))
        .route("/ingest/v1/blob/{source}/{filename}", get(transcribe::blob))
        .route("/sync/vocabulary/prompt", get(transcribe::prompt));

    // The Angular bundle, with SPA fallback to index.html. API-only when
    // STATIC_DIR is unset (`ng serve` proxies in dev).
    let app = if let Some(dir) = state.cfg.static_dir.clone() {
        let index = format!("{dir}/index.html");
        let serve = ServeDir::new(&dir).fallback(get(move |uri: axum::http::Uri| {
            let index = index.clone();
            async move { spa(&index, uri.path()).await }
        }));
        // Only the static service: API responses are neither.
        let serve = ServiceBuilder::new()
            .layer(axum::middleware::from_fn(cache_headers))
            .service(serve);
        app.fallback_service(serve)
    } else {
        app
    };

    app.with_state(state)
}
