//! Sending one Signal message through `signal-cli-rest-api`'s `/v2/send`, for a
//! row the viewer queued in `signal_outbox`.
//!
//! The linked device never receives what it sends itself (signal-cli feeds its
//! receive handlers from the receive loop alone), so the archived row comes from
//! the send's receipt: Signal's timestamp, and a result per recipient.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::db::{Db, SignalSend};
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

/// Read a response: a 201's body is a list with one response per kind of
/// recipient, and a send names one. A DM reaches one person, so a refusal for
/// them is no send; a group's send stands for the members it reached.
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
    // A list, one response per kind of recipient; a send names one recipient.
    let parsed = match parsed.as_array().map(Vec::as_slice) {
        Some([one]) => one.clone(),
        _ => {
            return Outcome::Refused(
                "signal-cli's answer is not one response; it may have gone".to_string(),
            );
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

/// Where a send goes: signal-cli-rest-api at `base`, as account `number`.
pub struct Target<'a> {
    pub http: &'a reqwest::Client,
    pub base: &'a str,
    pub number: &'a str,
    /// One call, above signal-cli's own retries for a slow recipient.
    pub timeout: Duration,
}

/// A send Signal took: when, whether the archive has it, and who it missed.
#[derive(Debug, PartialEq, Eq)]
pub struct Sent {
    pub ts: i64,
    pub archived: bool,
    pub missed: Option<String>,
}

/// Send one claimed row and, once Signal has taken it, archive it as the row a
/// sync of it would have written, marked `sent_here`. `Err` is why it did not
/// go, or may not have, in words for the reader.
pub async fn deliver(db: &Db, to: &Target<'_>, send: &SignalSend) -> Result<Sent, String> {
    let res = to
        .http
        .post(format!("{}/v2/send", to.base))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(request_body(to.number, send).to_string())
        .timeout(to.timeout)
        .send()
        .await
        .map_err(|e| format!("signal-cli did not answer; it may or may not have gone: {e}"))?;
    let status = res.status().as_u16();
    let body = res.bytes().await.map_err(|e| {
        format!("signal-cli's answer was cut off; it may or may not have gone: {e}")
    })?;
    let (ts, missed) = match outcome(&send.thread, status, &body) {
        Outcome::Sent { ts, missed } => (ts, missed),
        Outcome::Refused(why) => return Err(why),
    };
    let archived = match archive(db, send, ts).await {
        Ok(()) => true,
        Err(e) => {
            tracing::error!("signal send {} went but was not archived: {e:#}", send.id);
            false
        }
    };
    Ok(Sent {
        ts,
        archived,
        missed,
    })
}

/// The row: sent by this account, whose uuid the archive knows from the sync
/// messages only its own devices send.
async fn archive(db: &Db, send: &SignalSend, ts: i64) -> anyhow::Result<()> {
    let Some(me) = db.own_uuid().await? else {
        anyhow::bail!("this account's own uuid is not known yet");
    };
    let m = crate::parse::Message {
        thread_id: send.thread.clone(),
        sender: me,
        server_ts: ts,
        server_received_ts: None,
        server_delivered_ts: None,
        expires_in_seconds: None,
        body: Some(send.text.clone()),
        quote_target_ts: send.quote.as_ref().map(|q| q.ts),
        quote_author: send.quote.as_ref().map(|q| q.author.clone()),
        quote_text: None,
        is_outgoing: true,
        attachments: Vec::new(),
        styles: Vec::new(),
        mentions: Vec::new(),
        previews: Vec::new(),
        group_call: None,
    };
    db.upsert_conversation(&m.thread_id).await?;
    if let Some(id) = db.insert_message(&m).await? {
        db.mark_sent_here(id).await?;
    }
    Ok(())
}
