//! Voice messages turned into words by recall's transcriber, on the Mac.
//!
//! The Mac only ever calls out, so this is a queue it polls, in the shape
//! recall's `runner` already speaks (recall `runner/src/client.rs`): lease a
//! job, fetch its audio, hand back what the model heard. recall's one runner
//! process serves this queue with messages' own token whenever its own queue is
//! empty (`--messages-url`, recall 28d31d2); nothing reaches recall's archive.
//!
//! One row per audio attachment in `transcripts`: its id is the job id, and it
//! holds the lease, then the model's reply or its refusal. A row whose lease ran
//! out without an answer is leased again, up to `MAX_LEASES` times: a clip that
//! kills the shim would otherwise restart the shared Whisper process forever.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{AssertSqlSafe, MySqlPool};

use crate::archive::Origin;

/// How long a lease holds before the job is offered again: a transcription
/// takes seconds, so this only matters when the runner died mid-job.
const LEASE_MINUTES: i64 = 15;

/// How many leases a clip gets before it is retired unanswered, as recalld
/// retires a job.
const MAX_LEASES: i64 = 3;

/// The least average word confidence a clip needs for its words to be shown.
/// Whisper invents words over music and noise ("Thank you.", a mix of scripts),
/// and its `no_speech_prob` came back near zero for those too. Measured on the
/// first 31 clips: real speech averaged 0.77 and up (one at 0.98), inventions
/// 0.53 and down, two unclear clips 0.64 and 0.66. Below this, nothing is shown.
const CONFIDENT: f64 = 0.75;

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
    // A clip leased `MAX_LEASES` times without an answer is retired: whatever
    // it does to the shim, it is not handed out again.
    sqlx::query(
        r"UPDATE transcripts SET done_at = NOW(), error = 'no answer after the last lease'
          WHERE done_at IS NULL AND leases >= ? AND leased_at < NOW() - INTERVAL ? MINUTE",
    )
    .bind(MAX_LEASES)
    .bind(LEASE_MINUTES)
    .execute(pool)
    .await?;
    // The held audio of every origin, less what is done or leased; the newest of
    // each origin first, so a message just received is not behind the backlog.
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
          ORDER BY CAST(a.id AS UNSIGNED) DESC
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
        r"INSERT INTO transcripts (origin, attachment_id, leased_at, leases) VALUES (?, ?, NOW(), 1)
          ON DUPLICATE KEY UPDATE leased_at = NOW(), leases = leases + 1, id = LAST_INSERT_ID(id)",
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

/// The parts of the `asr` shim's reply read here (recall `audiocore::shim`).
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
    #[serde(default)]
    pub words: Vec<HeardWord>,
}

/// One word, and how sure the model was of it.
#[derive(Deserialize)]
pub struct HeardWord {
    pub probability: Option<f64>,
}

/// The clip's words, or `None` when the model was not sure enough of them on
/// average (`CONFIDENT`): all of them, or none, never a pick of segments.
pub fn words(heard: &Heard) -> Option<String> {
    let probs: Vec<f64> = heard
        .segments
        .iter()
        .flat_map(|s| &s.words)
        // A word without a probability says nothing either way.
        .filter_map(|w| w.probability)
        .collect();
    if probs.is_empty() || probs.iter().sum::<f64>() / (probs.len() as f64) < CONFIDENT {
        return None;
    }
    let text = heard
        .segments
        .iter()
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
    // Only the reply is stored; its words are read from it when shown (`texts`).
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

/// The words of each transcribed attachment among `ids`, for one origin: read
/// from the kept reply each time, so changing what counts as sure enough needs
/// no transcription again.
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
        "SELECT attachment_id, heard FROM transcripts
          WHERE origin = ? AND heard IS NOT NULL AND attachment_id IN ({placeholders})"
    );
    // Only `?` placeholders are spliced in; every value is bound.
    let mut q = sqlx::query_as::<_, (String, String)>(AssertSqlSafe(sql)).bind(origin_name(origin));
    for id in ids {
        q = q.bind(id);
    }
    let mut out = std::collections::HashMap::new();
    for (id, heard) in q.fetch_all(pool).await? {
        // A reply that no longer reads is shown as nothing, and logged.
        match serde_json::from_str::<Heard>(&heard) {
            Ok(h) => {
                if let Some(text) = words(&h) {
                    out.insert(id, text);
                }
            }
            Err(e) => tracing::warn!(
                "transcript of {origin_name} {id} does not read: {e}",
                origin_name = origin_name(origin)
            ),
        }
    }
    Ok(out)
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
