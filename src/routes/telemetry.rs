//! Client activity trace: what the browser sees and the API does not.
//!
//! What the browser sees and the API does not: taps that hit a cache, disabled
//! controls, route changes. Folded into the request log, so a session reads as
//! one timeline: `client-event kind=nav path=/conversations`, then
//! `client-event kind=tap label="Signal"`, then the request the tap caused.
//! Nothing is stored.

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
    /// A batch arrives at once, so only the client's clock orders it.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at: i64,
}

/// Most events accepted from one POST.
///
/// The client batches a handful; this bounds a buggy or hostile one.
const MAX_EVENTS: usize = 100;

/// Longest label kept, in characters.
///
/// Counted in chars, so a multi-byte glyph is never split.
const MAX_LABEL: usize = 160;

/// Format characters that are invisible, or that reorder what is displayed.
///
/// `char::is_control` covers only category Cc, so these are listed by hand:
/// zero-width characters, which make a label look empty, and bidi overrides,
/// which make a log line display something other than what it says. Not all of
/// category Cf.
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
/// The endpoint's security boundary: a newline in a label would forge log lines.
/// Control characters become spaces, whitespace runs collapse (which also
/// catches U+2028 and U+2029), and the result is capped in chars.
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
/// Always 204: the client neither reads nor retries. Auth-gated, so every line
/// is attributed.
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
