//! Whether the archive is readable, not merely whether this process is alive.
//!
//! Not the liveness target: `/healthz` stays a literal, since a liveness probe
//! that checks a dependency turns a database blip into a crashloop. Not wired to
//! readiness either, or a shared-database outage would pull every app backed by
//! it; hence `/healthz/deep` rather than `/readyz`. Unauthenticated, so the front
//! door can probe it; it discloses one bit.

use axum::extract::State;
use axum::http::StatusCode;
use std::time::Duration;

use crate::state::AppState;

/// How long the archive may take to answer before it counts as unreachable.
///
/// Short: under `plan-run`'s own 8 s curl timeout, so a hung database reads as
/// this app failing rather than the prober.
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(3);

/// GET /healthz/deep → 200 when the archive answers, 503 when it does not.
///
/// A status code, since the front door counts any 5xx as not serving. `SELECT 1`
/// proves the pool reaches and authenticates to the server; an empty archive is
/// not a broken one.
pub async fn deep(State(app): State<AppState>) -> (StatusCode, &'static str) {
    let probe = sqlx::query("SELECT 1").fetch_one(&app.pool);
    match tokio::time::timeout(ARCHIVE_TIMEOUT, probe).await {
        Ok(Ok(_)) => (StatusCode::OK, "archive reachable\n"),
        Ok(Err(e)) => {
            tracing::warn!("deep health: the archive refused a trivial query: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "archive unreachable\n")
        }
        Err(_) => {
            tracing::warn!(
                "deep health: the archive did not answer SELECT 1 within {}s",
                ARCHIVE_TIMEOUT.as_secs()
            );
            (StatusCode::SERVICE_UNAVAILABLE, "archive not answering\n")
        }
    }
}
