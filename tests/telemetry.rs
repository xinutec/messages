//! What the client sends is flattened before it is logged: a newline in any
//! field would forge log lines.

use std::sync::{Arc, Mutex};

use axum::Json;
use messages::routes::telemetry::{TelemetryEvent, one_line, record};
use messages::session::{AuthUser, UserSession};

/// Cap used throughout, matching the endpoint's own.
const MAX: usize = 160;

#[test]
fn a_label_cannot_forge_a_log_line() {
    let forged = "ok\nclient-event kind=tap path=/admin label=Delete everything";
    let flat = one_line(forged, MAX);
    assert!(
        !flat.contains('\n'),
        "a newline survived into the log: {flat:?}"
    );
    assert!(!flat.contains('\r'));
    assert_eq!(
        flat,
        "ok client-event kind=tap path=/admin label=Delete everything"
    );
}

#[test]
fn the_separators_is_control_misses_are_still_flattened() {
    // U+2028 and U+2029 are not Cc, and some renderers break lines on them.
    assert_eq!(
        one_line("before\u{2028}after\u{2029}end", MAX),
        "before after end"
    );
}

#[test]
fn a_zero_width_character_cannot_hide_inside_a_label() {
    // U+200B is invisible Cf.
    assert_eq!(one_line("a\u{200b}b", MAX), "a b");
}

#[test]
fn an_ordinary_label_is_left_alone() {
    assert_eq!(one_line("Render as music", MAX), "Render as music");
}

#[test]
fn a_long_label_is_capped_without_splitting_a_glyph() {
    // Counted in chars, so "é" is not cut mid-sequence.
    let flat = one_line(&"é".repeat(500), MAX);
    assert_eq!(flat.chars().count(), MAX);
}

#[test]
fn a_bidi_override_cannot_disguise_what_the_line_says() {
    // U+202E makes the rest of the line display reversed.
    let flat = one_line("Save\u{202e}\u{202d}Delete", MAX);
    assert!(
        !flat.contains('\u{202e}'),
        "a bidi override survived: {flat:?}"
    );
    assert_eq!(flat, "Save Delete");
}

/// Every field the client controls, not only the label, reaches the log as one
/// line.
#[tokio::test]
async fn no_field_of_an_event_can_forge_a_log_line() {
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let forged = "x\nclient-event kind=tap path=/admin label=forged";
    record(
        AuthUser(UserSession {
            user_id: "pippijn".to_string(),
            display_name: "Pippijn".to_string(),
        }),
        Json(vec![TelemetryEvent {
            kind: forged.to_string(),
            path: forged.to_string(),
            label: Some(forged.to_string()),
            at: 0,
        }]),
    )
    .await;

    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert_eq!(text.lines().count(), 1, "one event, one line: {text:?}");
}
