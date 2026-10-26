//! Sending one Signal message through `signal-cli-rest-api`'s `/v2/send`, for a
//! row the viewer queued in `signal_outbox`.
//!
//! The linked device never receives what it sends itself (signal-cli feeds its
//! receive handlers from the receive loop alone), so the archived row comes from
//! the send's receipt: Signal's timestamp, and a result per recipient.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::db::SignalSend;
use crate::parse::ThreadId;

/// How `/v2/send` names a thread: a DM by the contact's uuid, a group by
/// `group.` and the base64 of signal-cli's internal id, which the archive keys
/// groups by.
pub fn recipient(thread: &ThreadId) -> String {
    match thread {
        ThreadId::Dm(uuid) => uuid.clone(),
        ThreadId::Group(internal_id) => format!("group.{}", STANDARD.encode(internal_id)),
    }
}

/// The request for `send`, from `number` (the account, E.164).
pub fn request_body(number: &str, send: &SignalSend) -> Value {
    let mut body = json!({
        "number": number,
        "recipients": [recipient(&send.thread)],
        "message": send.text,
    });
    if let Some(q) = &send.quote {
        body["quote_timestamp"] = json!(q.ts);
        body["quote_author"] = json!(q.author);
        if let Some(text) = &q.text {
            body["quote_message"] = json!(text);
        }
    }
    body
}

/// What `/v2/send` answered.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Signal took it at `ts`. `missed` names the recipients it did not reach,
    /// with Signal's reason, as a note.
    Sent { ts: i64, missed: Option<String> },
    /// It did not go, and why.
    Refused(String),
}

/// Read a response. A DM reaches one person, so a refusal for them is no send;
/// a group's send stands for the members it reached.
pub fn outcome(thread: &ThreadId, status: u16, body: &[u8]) -> Outcome {
    let parsed = serde_json::from_slice::<Value>(body);
    if status != 201 {
        // An error page need not be JSON; its status then says what there is.
        let said = match &parsed {
            Ok(v) => v.get("error").and_then(Value::as_str).map(str::to_string),
            Err(_) => None,
        };
        return Outcome::Refused(said.unwrap_or_else(|| format!("signal-cli answered {status}")));
    }
    // A 201 means Signal took it, so an answer it cannot read is no refusal.
    let parsed = match parsed {
        Ok(v) => v,
        Err(e) => {
            return Outcome::Refused(format!(
                "signal-cli's answer is not JSON; it may have gone: {e}"
            ));
        }
    };
    let Some(ts) = parsed
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|t| t.parse::<i64>().ok())
    else {
        return Outcome::Refused("signal-cli gave no timestamp; it may have gone".to_string());
    };
    let missed: Vec<String> = parsed
        .pointer("/errors/recipients")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|r| {
            let who = ["uuid", "number", "username"]
                .iter()
                .find_map(|k| r.get(*k).and_then(Value::as_str))
                .unwrap_or("someone");
            let why = r.get("reason").and_then(Value::as_str).unwrap_or("unknown");
            format!("{who} ({why})")
        })
        .collect();
    match (thread, missed.is_empty()) {
        (_, true) => Outcome::Sent { ts, missed: None },
        (ThreadId::Dm(_), false) => {
            Outcome::Refused(format!("not delivered: {}", missed.join(", ")))
        }
        (ThreadId::Group(_), false) => Outcome::Sent {
            ts,
            missed: Some(format!("not delivered to {}", missed.join(", "))),
        },
    }
}
