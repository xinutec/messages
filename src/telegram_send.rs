//! Sending one Telegram message, as the user, through the feed that holds the
//! session.
//!
//! The feed takes no connections, so this queues a row in `telegram_outbox` and
//! waits while the feed polls it (archiver migration v59 has the states). The
//! feed sends, reads the message back from Telegram and stores that, so the
//! archive never holds a message only this app says was sent: unlike Signal
//! (`routes::api::send`), Telegram itself answers for it.
//!
//! The recipient is a conversation the archive holds, a DM or a group: the
//! queueing insert reads it from `telegram_conversations`, so a request cannot
//! address anyone else, nor post to a broadcast.

use std::time::Duration;

use anyhow::Result;
use sqlx::MySqlPool;

/// How long a reader waits for the feed. Well inside the feed's own
/// `OUTBOX_FRESH_SECS`, so a send it has not reached by then is withdrawn first.
const WAIT: Duration = Duration::from_secs(20);

/// How often the row is read while waiting.
const POLL: Duration = Duration::from_millis(250);

/// What became of a send.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Telegram has it. `archived` is false when the feed could not store it
    /// yet; the note says why.
    Sent { archived: bool },
    /// It did not go, and why.
    Refused(String),
    /// The feed took it and has not answered: it may yet go, or have gone.
    Unanswered,
}

/// Queue `text` for a DM or group the archive holds, or `None` for anything
/// else.
pub async fn queue(pool: &MySqlPool, conversation: &str, text: &str) -> Result<Option<i64>> {
    let Ok(id) = conversation.parse::<i64>() else {
        return Ok(None);
    };
    let queued = sqlx::query(
        "INSERT INTO telegram_outbox (conversation_id, text)
         SELECT id, ? FROM telegram_conversations WHERE id = ? AND kind IN ('dm', 'group')",
    )
    .bind(text)
    .bind(id)
    .execute(pool)
    .await?;
    if queued.rows_affected() != 1 {
        return Ok(None);
    }
    Ok(Some(i64::try_from(queued.last_insert_id())?))
}

/// Wait for the feed to answer row `id`, withdrawing it if the feed never
/// reached it.
pub async fn wait(pool: &MySqlPool, id: i64) -> Result<Outcome> {
    wait_for(pool, id, WAIT).await
}

/// [`wait`], for `patience` rather than [`WAIT`].
pub async fn wait_for(pool: &MySqlPool, id: i64, patience: Duration) -> Result<Outcome> {
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        if let Some(outcome) = answer(pool, id).await? {
            return Ok(outcome);
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    // Conditional, so it loses to a feed that has just claimed it.
    let withdrawn = sqlx::query(
        "UPDATE telegram_outbox SET state = 'failed', done_at = CURRENT_TIMESTAMP,
                note = 'the Telegram feed did not answer; not sent'
          WHERE id = ? AND state = 'queued'",
    )
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    if withdrawn == 1 {
        return Ok(Outcome::Refused(
            "The Telegram feed did not answer; not sent.".to_owned(),
        ));
    }
    Ok(answer(pool, id).await?.unwrap_or(Outcome::Unanswered))
}

/// The feed's answer for row `id`, or `None` while it has none.
async fn answer(pool: &MySqlPool, id: i64) -> Result<Option<Outcome>> {
    let (state, note): (String, Option<String>) =
        sqlx::query_as("SELECT CAST(state AS CHAR), note FROM telegram_outbox WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;
    Ok(match state.as_str() {
        "sent" => Some(Outcome::Sent {
            archived: note.is_none(),
        }),
        "failed" => Some(Outcome::Refused(
            note.unwrap_or_else(|| "not sent".to_owned()),
        )),
        _ => None,
    })
}
