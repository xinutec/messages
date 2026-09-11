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
    // ⚠ **A 404 is not an asset.** `SetResponseHeaderLayer::overriding` stamps
    // whatever the service returned, and a missing file answered with a year of
    // `immutable` is a client that will not ask for that name again this year.
    // Only a response that carried something may say how long it keeps.
    //
    // ⚠ NOT `!is_success()`. That excludes **304 Not Modified**, which must
    // carry the headers a 200 would so the client can refresh what it already
    // holds. Stripping it made every revalidated image a full re-fetch, and the
    // arriving bytes grew the thread AFTER it had scrolled to the bottom —
    // `thread-scroll.spec.ts` caught it at 271px off, a symptom with no visible
    // connection to a cache header.
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
/// ⚠ **A missing FILE must not be handed the page, and the mistake is
/// invisible**: the wrong answer is a `200`, so a browser that asked for a
/// woff2 and got HTML renders broken icons and reports nothing anywhere.
/// Measured 2026-09-08 — `/media/nope.woff2` answered `200 text/html` (#1478).
///
/// ⚠ This app was not in that task's list. The list came from probing two
/// names; asking every host in `frontdoor.json` found eight, this one among
/// them — a set REPORTED rather than DERIVED, which is what #881 was reopened
/// for one week earlier.
///
/// The test is a dot in the last path segment. It is a heuristic, and the
/// alternative — enumerating the bundle's own asset names — would have to be
/// rebuilt whenever `ng build` changes a hash.
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
            // STATIC_DIR set with no index is a misconfigured deployment, and
            // saying so beats serving an empty page that looks like the app.
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
        // The app's only write, and the only route another person can observe
        // the effect of. IRC only; see `api::send`.
        .route("/conversations/{origin}/{id}/send", post(api::send))
        .route("/attachments/{id}", get(api::attachment))
        .route("/link-images/{id}", get(api::link_image))
        .route("/link-images/{id}/request", post(api::request_link_image))
        .route("/search", get(api::search))
        // What the person did, folded into the same log as what the API saw.
        .route("/telemetry", post(telemetry::record));

    let app = Router::new()
        // ⚠ **DELIBERATELY DUMB, and it must stay that way.** This is kubelet's
        // LIVENESS target (`apps/messages.dhall`: initialDelaySeconds 5,
        // periodSeconds 20). A liveness probe that checks a dependency turns a
        // database blip into a crashloop — kubelet kills the pod for something
        // restarting cannot fix, and the restarts make the dependency's load
        // worse. Whether the archive is READABLE is `/api/health`'s question.
        .route("/healthz", get(|| async { "ok" }))
        // The dependency question the line above must never learn to ask.
        .route("/healthz/deep", get(health::deep))
        .route("/login", get(auth::login))
        .route("/auth/callback", get(auth::callback))
        .route("/logout", post(auth::logout))
        .nest("/api", api);

    // Serve the built Angular bundle (single origin), SPA-fallback to index.html.
    // API-only when STATIC_DIR is unset (dev: `ng serve` proxies).
    let app = if let Some(dir) = state.cfg.static_dir.clone() {
        let index = format!("{dir}/index.html");
        let serve = ServeDir::new(&dir).fallback(get(move |uri: axum::http::Uri| {
            let index = index.clone();
            async move { spa(&index, uri.path()) }
        }));
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
