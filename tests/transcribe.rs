//! What is shown of what recall's transcriber heard: a clip's words when the
//! model was sure enough of them on average, and nothing otherwise. Whisper
//! invents over music and noise ("Thank you."), with its silence estimate near
//! zero all the same; the numbers here are the shapes measured on real clips.

use messages::transcribe::{Heard, words};

/// A clip of segments, each `(text, [word probabilities])`.
fn clip(segments: &[(&str, &[f64])]) -> Heard {
    let segments: Vec<_> = segments
        .iter()
        .map(|(text, probs)| {
            serde_json::json!({
                "text": text,
                "no_speech_prob": 0.0,
                "words": probs.iter().map(|p| serde_json::json!({ "start": 0.0, "end": 0.1, "text": "w", "probability": p })).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::from_value(serde_json::json!({ "language": "en", "segments": segments })).unwrap()
}

#[test]
fn a_clip_the_model_was_sure_of_shows_whole() {
    let h = clip(&[(" I love you.", &[0.9, 0.95, 0.97]), (" So...", &[0.8])]);
    assert_eq!(words(&h).as_deref(), Some("I love you. So..."));
}

/// The commonest invention, at the confidence it came back with on music.
#[test]
fn a_hallucinated_thank_you_shows_nothing() {
    assert_eq!(words(&clip(&[(" Thank you.", &[0.53, 0.57])])), None);
}

/// All or nothing: a sure segment does not carry an unsure clip.
#[test]
fn an_unsure_clip_shows_nothing_even_where_one_segment_was_sure() {
    let h = clip(&[
        (" Okay.", &[0.95]),
        (" Carl Topo", &[0.01, 0.02]),
        (" .", &[0.06]),
    ]);
    assert_eq!(words(&h), None);
}

#[test]
fn a_clip_with_no_words_shows_nothing() {
    assert_eq!(words(&clip(&[])), None);
    assert_eq!(words(&clip(&[(" ", &[0.99])])), None);
}

/// The reply recall's own shim tests produce (recall
/// `tests/fixtures/shim/transcribe-reply.json`), read as it is.
#[test]
fn recalls_reply_shape_reads() {
    let reply = r#"{"language":"en","language_confidence":null,"segments":[{"start":0.0,"end":1.5,"text":" hello there","avg_logprob":-0.25,"no_speech_prob":0.01,"confidence":0.77,"words":[{"start":0.0,"end":0.5,"text":" hello","probability":0.875},{"start":0.5,"end":1.5,"text":" there","probability":0.75}]}]}"#;
    let h: Heard = serde_json::from_str(reply).unwrap();
    assert_eq!(words(&h).as_deref(), Some("hello there"));
}
