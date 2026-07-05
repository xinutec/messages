//! HTTP routing table.

pub mod api;
pub mod auth;

use axum::Router;
use axum::routing::{get, post};
use tower_http::services::{ServeDir, ServeFile};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/me", get(api::me))
        .route("/conversations", get(api::conversations))
        .route("/conversations/{origin}/{id}/messages", get(api::messages))
        .route("/attachments/{id}", get(api::attachment))
        .route("/search", get(api::search));

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
        app.fallback_service(serve)
    } else {
        app
    };

    app.with_state(state)
}
