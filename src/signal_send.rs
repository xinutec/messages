//! Sending one Signal message, as the user, through the ingester that holds the
//! linked device.
//!
//! This queues a row in `signal_outbox` and waits while the ingester polls it
//! ([`crate::outbox`]). The linked device never receives its own sends, so the
//! ingester archives the message from Signal's receipt and marks it
//! `sent_here`: the viewer shows it as sent from this app, confirmed by Signal.
//!
//! The recipient is a conversation the archive holds: the queueing insert reads
//! it from `conversations`, so a request cannot address anyone else.

use anyhow::Result;
use sqlx::MySqlPool;

/// Queue `text` for a thread the archive holds, answering the message whose
/// archive row is `reply_to` when given. `None` for anything else, a `reply_to`
/// outside this thread included.
pub async fn queue(
    pool: &MySqlPool,
    thread: &str,
    text: &str,
    reply_to: Option<i64>,
) -> Result<Option<i64>> {
    // Not a thread key at all: no conversation to look for.
    if !(thread.starts_with("dm:") || thread.starts_with("group:")) {
        return Ok(None);
    }
    // The row is looked up in this thread, so a reply cannot reach into another;
    // Signal names the answered message by its timestamp.
    let queued = sqlx::query(
        "INSERT INTO signal_outbox (thread_id, text, quote_ts)
         SELECT c.thread_id, ?, m.server_ts
           FROM conversations c
           LEFT JOIN messages m ON m.thread_id = c.thread_id AND m.id = ?
          WHERE c.thread_id = ?
            AND (? IS NULL OR m.server_ts IS NOT NULL)",
    )
    .bind(text)
    .bind(reply_to)
    .bind(thread)
    .bind(reply_to)
    .execute(pool)
    .await?;
    if queued.rows_affected() != 1 {
        return Ok(None);
    }
    Ok(Some(i64::try_from(queued.last_insert_id())?))
}
