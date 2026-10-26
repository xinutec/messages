//! What a Signal send asks `/v2/send` and what its answer means
//! (`signal_archiver::send`), against the shapes signal-cli-rest-api 0.101 sends:
//! a 201's body is a list, one response per kind of recipient (`api.go` writes
//! `SendV2`'s `*[]SendMessageResponse` as it is).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use signal_archiver::db::{SignalQuote, SignalSend};
use signal_archiver::parse::ThreadId;
use signal_archiver::send::{Outcome, outcome, recipient, request_body};

fn dm() -> ThreadId {
    ThreadId::Dm("0c1f6a3e-6b1d-4d1e-9a4a-2f1c3b5d7e9f".into())
}

/// A group id as signal-cli reports it: base64 of the 32 raw bytes.
fn group() -> ThreadId {
    ThreadId::Group(STANDARD.encode([7u8; 32]))
}

#[test]
fn a_dm_goes_to_the_uuid_and_a_group_to_the_double_base64_of_its_id() {
    assert_eq!(recipient(&dm()), "0c1f6a3e-6b1d-4d1e-9a4a-2f1c3b5d7e9f");
    let r = recipient(&group());
    let inner = STANDARD
        .decode(r.strip_prefix("group.").expect("a group prefix"))
        .unwrap();
    // signal-cli-rest-api's own check: twice decoded, 32 bytes.
    assert_eq!(STANDARD.decode(inner).unwrap().len(), 32);
}

#[test]
fn a_reply_carries_the_quote_signal_wants() {
    let send = SignalSend {
        id: 1,
        thread: dm(),
        text: "yes".into(),
        quote: Some(SignalQuote {
            ts: 1_700_000_000_000,
            author: "ab-cd".into(),
            text: Some("shall we?".into()),
        }),
    };
    let body = request_body("+10000000000", &send);
    assert_eq!(body["message"], "yes");
    assert_eq!(
        body["recipients"][0],
        "0c1f6a3e-6b1d-4d1e-9a4a-2f1c3b5d7e9f"
    );
    assert_eq!(body["quote_timestamp"], 1_700_000_000_000i64);
    assert_eq!(body["quote_author"], "ab-cd");
    assert_eq!(body["quote_message"], "shall we?");
    let plain = request_body(
        "+10000000000",
        &SignalSend {
            quote: None,
            ..send
        },
    );
    assert!(plain.get("quote_timestamp").is_none());
}

#[test]
fn a_201_with_a_timestamp_is_sent() {
    assert_eq!(
        outcome(&dm(), 201, br#"[{"timestamp":"1700000000123"}]"#),
        Outcome::Sent {
            ts: 1_700_000_000_123,
            missed: None
        }
    );
}

#[test]
fn a_dm_refused_for_its_one_recipient_did_not_go() {
    let body = br#"[{"timestamp":"1700000000123","errors":{"recipients":[{"uuid":"ab-cd","reason":"UNREGISTERED_FAILURE"}]}}]"#;
    assert_eq!(
        outcome(&dm(), 201, body),
        Outcome::Refused("not delivered: ab-cd (UNREGISTERED_FAILURE)".into())
    );
}

#[test]
fn a_group_send_stands_for_those_it_reached_and_names_the_rest() {
    let body = br#"[{"timestamp":"1700000000123","errors":{"recipients":[{"number":"+3100","reason":"IDENTITY_FAILURE"}]}}]"#;
    assert_eq!(
        outcome(&group(), 201, body),
        Outcome::Sent {
            ts: 1_700_000_000_123,
            missed: Some("not delivered to +3100 (IDENTITY_FAILURE)".into())
        }
    );
}

#[test]
fn an_unreadable_answer_to_a_send_signal_took_does_not_say_it_failed() {
    let Outcome::Refused(why) = outcome(&dm(), 201, b"<html>") else {
        panic!("no timestamp to archive it by");
    };
    assert!(why.contains("may have gone"), "{why}");
}

/// One recipient per send, so one response: anything else is not read as a send.
#[test]
fn an_answer_that_is_not_one_response_is_not_a_send() {
    for body in [
        &br#"{"timestamp":"1700000000123"}"#[..],
        br#"[]"#,
        br#"[{"timestamp":"1"},{"timestamp":"2"}]"#,
    ] {
        let Outcome::Refused(why) = outcome(&dm(), 201, body) else {
            panic!("read as a send: {}", String::from_utf8_lossy(body));
        };
        assert!(why.contains("may have gone"), "{why}");
    }
}

#[test]
fn an_error_status_says_why() {
    assert_eq!(
        outcome(&dm(), 400, br#"{"error":"Untrusted Identity for ab-cd"}"#),
        Outcome::Refused("Untrusted Identity for ab-cd".into())
    );
    assert_eq!(
        outcome(&dm(), 500, b"oops"),
        Outcome::Refused("signal-cli answered 500".into())
    );
    assert_eq!(
        outcome(&dm(), 201, br#"[{}]"#),
        Outcome::Refused("signal-cli gave no timestamp; it may have gone".into())
    );
}
