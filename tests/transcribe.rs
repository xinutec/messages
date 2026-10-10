//! What is kept of what recall's transcriber heard: its words, less what the
//! model rated as likely silence, where Whisper invents ("Thank you.").

use messages::transcribe::{Heard, words};

fn heard(segments: serde_json::Value) -> Heard {
    serde_json::from_value(serde_json::json!({ "language": "en", "segments": segments })).unwrap()
}

#[test]
fn the_words_are_kept_in_order() {
    let h = heard(serde_json::json!([
        { "start": 0.0, "end": 1.5, "text": " hello there", "no_speech_prob": 0.01 },
        { "start": 1.5, "end": 3.0, "text": " how are you", "no_speech_prob": 0.05 },
    ]));
    assert_eq!(words(&h).as_deref(), Some("hello there how are you"));
}

#[test]
fn what_the_model_calls_silence_is_dropped() {
    let h = heard(serde_json::json!([
        { "text": " hello", "no_speech_prob": 0.1 },
        { "text": " Thank you.", "no_speech_prob": 0.9 },
    ]));
    assert_eq!(words(&h).as_deref(), Some("hello"));
}

#[test]
fn a_clip_of_only_silence_has_no_words() {
    let h = heard(serde_json::json!([{ "text": " Thank you.", "no_speech_prob": 0.95 }]));
    assert_eq!(words(&h), None);
    assert_eq!(words(&heard(serde_json::json!([]))), None);
}

/// The reply recall's own shim tests produce (recall
/// `tests/fixtures/shim/transcribe-reply.json`), read as it is.
#[test]
fn recalls_reply_shape_reads() {
    let reply = r#"{"language":"en","language_confidence":null,"segments":[{"start":0.0,"end":1.5,"text":" hello there","avg_logprob":-0.25,"no_speech_prob":0.01,"confidence":0.77,"words":[{"start":0.0,"end":0.5,"text":" hello","probability":0.875}]}]}"#;
    let h: Heard = serde_json::from_str(reply).unwrap();
    assert_eq!(words(&h).as_deref(), Some("hello there"));
}
