//! Voice messages turned into words by recall's transcriber, on the Mac.
//!
//! The Mac only ever calls out, so this is a queue it polls, in the shape
//! recall's `runner` already speaks (recall `runner/src/client.rs`): lease a
//! job, fetch its audio, hand back what the model heard. A second runner,
//! pointed here with its own token, does the work; nothing reaches recall's own
//! archive.
//!
//! One row per audio attachment in `transcripts`: its id is the job id, and it
//! holds the lease, then the words or the model's refusal. A row whose lease ran
//! out without an answer is leased again.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{AssertSqlSafe, MySqlPool};

use crate::archive::Origin;

/// How long a lease holds before the job is offered again: a transcription
/// takes seconds, so this only matters when the runner died mid-job.
const LEASE_MINUTES: i64 = 15;

/// A segment the model itself rates likelier silence than speech is dropped:
/// Whisper invents words over quiet ("Thank you."), and recall's rule is that
/// silence is transcribed as nothing.
const NO_SPEECH: f64 = 0.6;

/// One job, as recall's runner reads it (`runner::client::Job`).
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: i64,
    pub kind: &'static str,
    /// The attachment's id within its origin.
    pub filename: String,
    /// The origin, which the blob route takes back.
    pub source: String,
}

/// Lease the next audio attachment that has no words yet and no live lease.
pub async fn lease(pool: &MySqlPool) -> Result<Option<Job>> {
    // The held audio of every origin, less what is done or leased.
    let next: Option<(String, String)> = sqlx::query_as(
        r"SELECT a.origin, a.id FROM (
              SELECT 'signal' AS origin, CAST(id AS CHAR) AS id FROM attachments
               WHERE content_type LIKE 'audio/%' AND stored_path IS NOT NULL
              UNION ALL
              SELECT 'gchat', CAST(id AS CHAR) FROM gchat_attachments
               WHERE mime LIKE 'audio/%' AND stored_path IS NOT NULL
              UNION ALL
              SELECT 'telegram', CAST(m.id AS CHAR) FROM telegram_messages m
                JOIN telegram_media d ON d.conversation_id = m.conversation_id AND d.msg_id = m.msg_id
               WHERE d.content_type LIKE 'audio/%' AND d.state = 'stored'
          ) a
          LEFT JOIN transcripts t ON t.origin = a.origin AND t.attachment_id = a.id
          WHERE t.id IS NULL
             OR (t.done_at IS NULL AND t.leased_at < NOW() - INTERVAL ? MINUTE)
          LIMIT 1",
    )
    .bind(LEASE_MINUTES)
    .fetch_optional(pool)
    .await?;
    let Some((origin, id)) = next else {
        return Ok(None);
    };
    // `LAST_INSERT_ID(id)` hands back the existing row's id on a re-lease too.
    let job_id = sqlx::query(
        r"INSERT INTO transcripts (origin, attachment_id, leased_at) VALUES (?, ?, NOW())
          ON DUPLICATE KEY UPDATE leased_at = NOW(), id = LAST_INSERT_ID(id)",
    )
    .bind(&origin)
    .bind(&id)
    .execute(pool)
    .await?
    .last_insert_id();
    Ok(Some(Job {
        id: i64::try_from(job_id)?,
        kind: "transcribe-segment",
        filename: id,
        source: origin,
    }))
}

/// What the runner hands back (`runner`'s `Stored`): the shim's reply as it
/// sent it, or why it refused the clip.
#[derive(Deserialize)]
pub struct Finished {
    pub ok: bool,
    /// The shim's reply, kept whole beside the words taken from it (`heard`), so
    /// what counts as speech can be measured and retuned without asking again.
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// The parts of the `asr` shim's reply kept here (recall `audiocore::shim`).
#[derive(Deserialize)]
pub struct Heard {
    pub language: Option<String>,
    #[serde(default)]
    pub segments: Vec<HeardSegment>,
}

/// One stretch of what was heard.
#[derive(Deserialize)]
pub struct HeardSegment {
    #[serde(default)]
    pub text: String,
    pub no_speech_prob: Option<f64>,
}

/// The words heard, less what the model rated as likely silence; `None` when
/// that leaves nothing.
pub fn words(heard: &Heard) -> Option<String> {
    let text = heard
        .segments
        .iter()
        .filter(|s| s.no_speech_prob.is_none_or(|p| p < NO_SPEECH))
        .map(|s| s.text.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

/// Record a job's outcome. `false` for a job this queue never handed out.
pub async fn finish(pool: &MySqlPool, id: i64, done: &Finished) -> Result<bool> {
    let raw = done.result.as_ref().filter(|_| done.ok);
    let heard = raw
        .map(|r| serde_json::from_value::<Heard>(r.clone()))
        .transpose()?;
    // No words are stored or shown yet: what counts as speech is being measured
    // on the kept replies first (music came back as "Thank you.").
    let language = heard.as_ref().and_then(|h| h.language.clone());
    let error = if done.ok {
        None
    } else {
        Some(done.error.clone().unwrap_or_else(|| "refused".to_owned()))
    };
    let updated = sqlx::query(
        "UPDATE transcripts SET done_at = NOW(), language = ?, error = ?, heard = ? WHERE id = ?",
    )
    .bind(language)
    .bind(error.map(|e| e.chars().take(255).collect::<String>()))
    .bind(raw.map(serde_json::Value::to_string))
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// The words of each transcribed attachment among `ids`, for one origin.
pub async fn texts(
    pool: &MySqlPool,
    origin: Origin,
    ids: &[String],
) -> Result<std::collections::HashMap<String, String>> {
    if ids.is_empty() {
        return Ok(Default::default());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT attachment_id, text FROM transcripts
          WHERE origin = ? AND text IS NOT NULL AND attachment_id IN ({placeholders})"
    );
    // Only `?` placeholders are spliced in; every value is bound.
    let mut q = sqlx::query_as::<_, (String, String)>(AssertSqlSafe(sql)).bind(origin_name(origin));
    for id in ids {
        q = q.bind(id);
    }
    Ok(q.fetch_all(pool).await?.into_iter().collect())
}

/// An origin as `transcripts.origin` spells it, which is how the API does.
fn origin_name(origin: Origin) -> &'static str {
    match origin {
        Origin::Signal => "signal",
        Origin::Gchat => "gchat",
        Origin::Irc => "irc",
        Origin::Telegram => "telegram",
    }
}
