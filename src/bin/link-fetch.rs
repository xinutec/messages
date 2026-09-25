//! The fetch service: the one part of this app that reaches the open internet,
//! and it holds no credentials, database or disk, so a hostile page finds
//! nothing behind it. A service rather than a job, because a reader taps and
//! waits.
//!
//!     POST /fetch   {"url": "https://…"}
//!       200 + image bytes   a picture, with its Content-Type
//!       204                 reached it; not a picture we may inline
//!       502                 could not reach it, or it broke the limits

use anyhow::Result;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use messages::link_fetch::{Limits, fetch_picture};
use serde::Deserialize;
use tracing_subscriber::EnvFilter;
use url::Url;

#[derive(Clone)]
struct Svc {
    client: reqwest::Client,
    limits: Limits,
}

#[derive(Deserialize)]
struct FetchRequest {
    url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // A user agent, since the other end sees it.
    let client = reqwest::Client::builder()
        .user_agent("xinutec-messages/1.0 (link preview; one fetch per link, on request)")
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()?;

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/fetch", post(fetch))
        .with_state(Svc {
            client,
            limits: Limits::default(),
        });

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("link fetcher listening on {port}; no database, no volume, no credentials");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn fetch(State(svc): State<Svc>, Json(req): Json<FetchRequest>) -> Response {
    let Ok(url) = Url::parse(&req.url) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    // Its own rule, rather than trusting the caller: http(s) only.
    if !matches!(url.scheme(), "http" | "https") {
        return StatusCode::BAD_REQUEST.into_response();
    }

    match fetch_picture(&svc.client, &url, svc.limits).await {
        Ok(Some((bytes, content_type))) => {
            tracing::info!("fetched {} bytes of {content_type}", bytes.len());
            ([(header::CONTENT_TYPE, content_type)], bytes).into_response()
        }
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            // The reason travels back to be recorded.
            tracing::info!("could not fetch: {e}");
            (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
    }
}
