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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct TelemetryEvent {
    pub kind: String,
    pub path: String,
    #[serde(default)]
    pub label: Option<String>,
    /// The client's clock, in epoch milliseconds.
    ///
    /// Kept because a batch arrives all at once, so the server's receive time
    /// cannot order the events inside it and the client's can.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
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

/// Format characters that are invisible, or that reorder what is displayed.
///
/// `char::is_control` covers categories Cc and nothing else, and Rust's std has
/// no Unicode category table — so these are named explicitly. Two reasons they
/// matter here, and the second is the sharper one:
///
/// - **Zero-width characters** (U+200B, U+FEFF, the word joiners) are invisible,
///   so a label made of them reads as empty while occupying the whole cap.
/// - **Bidi overrides** (U+202A–202E, U+2066–2069) reorder the *rendering* of
///   the text around them. A log line containing one can be made to display
///   something other than what it says — the Trojan Source trick, pointed at the
///   record rather than at source code.
///
/// A deny-list of what can deceive rather than all of category Cf, because
/// pulling a Unicode tables crate in for this would be disproportionate. Stated
/// so the limit is known rather than assumed.
fn is_deceptive_format(c: char) -> bool {
    matches!(c,
        '\u{00ad}'
        | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}'
        | '\u{feff}'
    )
}

/// Flatten a client-supplied label to a single harmless log field.
///
/// **This is the security boundary of the endpoint, not tidiness.** A label is
/// verbatim UI text and it is written into a log line as `label=…`. A label
/// containing a newline therefore forges *whole log lines* — including further
/// `client-event` lines attributed to someone else, or lines that look like they
/// came from another component entirely. The log stops being evidence, which is
/// the one thing it exists to be.
///
/// Control characters become spaces, runs of whitespace collapse, and the result
/// is capped. `char::is_control` covers C0 and C1 but *not* U+2028 and U+2029,
/// which end a line in some renderers; `split_whitespace` catches those, so the
/// two passes together cover both. Capped in `chars` rather than bytes so a
/// multi-byte glyph is never split down the middle.
///
/// Public so `tests/telemetry.rs` can exercise it directly: it is the one part
/// of this endpoint an attacker chooses the input to.
pub fn one_line(label: &str, max: usize) -> String {
    let unbroken: String = label
        .chars()
        .map(|c| {
            if c.is_control() || is_deceptive_format(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    unbroken
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

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
        let label = one_line(&e.label.unwrap_or_default(), MAX_LABEL);
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
