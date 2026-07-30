//! Client activity trace: what the browser sees and the API does not.
//!
//! **Why this exists, and it is not analytics.** The per-request trace already
//! logs every API call, and that was treated as sufficient across the fleet for
//! a long time. It is not: a tap that hits a cache, a control that was disabled,
//! a screen that rendered wrong — none of it reaches the server, so none of it
//! can be diagnosed afterwards from a report like "I pressed it and nothing
//! happened".
//!
//! The events fold into the **same** log stream as the API requests, so a
//! session reads as one timeline: `client-event kind=nav path=/conversations`, then
//! `client-event kind=tap label="Signal"`, then the `GET /api/conversations/…
//! 200` the tap caused.
//!
//! **There is no storage here.** These are logs, not data. The endpoint moves
//! the client's events into the backend log and forgets them.
//!
//! Ported from the `life` app, where this has run since 2026-07-17.

use axum::Json;
use axum::http::StatusCode;
use serde::Deserialize;

use crate::session::AuthUser;

/// One thing that happened in the client.
///
/// `kind` is `nav` for a route change, where `label` is absent, or `tap` for a
/// control, where `label` is its visible text, verbatim.
#[derive(Debug, Deserialize)]
pub struct TelemetryEvent {
    pub kind: String,
    pub path: String,
    #[serde(default)]
    pub label: Option<String>,
    /// The client's clock, in epoch milliseconds.
    ///
    /// Kept because a batch arrives all at once, so the server's receive time
    /// cannot order the events inside it and the client's can.
    pub at: i64,
}

/// Most events accepted from one POST.
///
/// The real client batches a handful at a time; this stops a buggy or hostile
/// one turning a single request into a log flood.
const MAX_EVENTS: usize = 100;

/// Longest label kept, in characters.
///
/// Labels are verbatim UI text, so a pathological one would otherwise bloat a
/// log line. Counted in `chars` rather than bytes so a multi-byte glyph is never
/// split down the middle.
const MAX_LABEL: usize = 160;

/// `POST /api/telemetry` — fold the client's events into the log stream.
///
/// Always 204. Telemetry is best-effort: the client neither reads the response
/// nor retries, because a trace that interferes with the app it observes is
/// worse than no trace. Auth-gated, so every line is attributed and this is not
/// an open log-write for anyone who finds the URL.
pub async fn record(
    AuthUser(user): AuthUser,
    Json(events): Json<Vec<TelemetryEvent>>,
) -> StatusCode {
    for e in events.into_iter().take(MAX_EVENTS) {
        let label: String = e
            .label
            .unwrap_or_default()
            .chars()
            .take(MAX_LABEL)
            .collect();
        tracing::info!(
            user = %user.user_id,
            kind = %e.kind,
            path = %e.path,
            label = %label,
            at = e.at,
            "client-event"
        );
    }
    StatusCode::NO_CONTENT
}
