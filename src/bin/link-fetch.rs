//! The fetch service: the one thing in this app that reaches the open internet,
//! and the one thing that knows nothing else.
//!
//! ⚠ **IT HOLDS NO CREDENTIALS, NO DATABASE AND NO DISK.** That is the whole
//! design. It follows links strangers wrote into a chat years ago, so it is the
//! component most likely to meet something hostile — and when it does, there is
//! nothing behind it to take: the archive's credentials live in the web pod, the
//! stored pictures live on the web pod's volume, and this process has neither.
//! The worst a compromised fetcher can do is lie about the bytes of a picture
//! somebody asked for.
//!
//! It is also why this is a service rather than a scheduled job. A person taps
//! and waits; a CronJob's floor is one minute, which is not a chat client. The
//! web pod asks, this answers, the picture appears.
//!
//!     POST /fetch   {"url": "https://…"}
//!       200 + image bytes   a picture, with its Content-Type
//!       204                 reached it; not a picture we may inline
//!       502                 could not reach it, or it broke the limits

use std::time::Duration;

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

    // ⚠ A NAME, because the other end sees it. A fetch with no user agent reads
    // as a scraper, and this one is a person's chat client showing them a
    // picture they were sent.
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
    // ⚠ Only what a browser would have followed anyway. The caller takes URLs off
    // messages in the archive, but this service states its own rule rather than
    // trusting that: a scheme it does not serve is not a link it fetches.
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
            // The reason travels back so the caller can record it: "failed" with
            // no reason is the kind of row that gets retried by hand for ever.
            tracing::info!("could not fetch: {e}");
            (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
    }
}

/// Timeouts are the service's own business; the caller sets its own on top.
const _: Duration = Duration::from_secs(0);
