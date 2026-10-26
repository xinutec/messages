//! Sending one Telegram message, as the user, through the feed that holds the
//! session.
//!
//! The feed takes no connections, so this queues a row in `telegram_outbox` and
//! waits while the feed polls it ([`crate::outbox`]). The feed sends, reads the
//! message back from Telegram and stores that: the archived row is Telegram's
//! own copy, where a Signal send's is its receipt ([`crate::signal_send`]).
//!
//! The recipient is a conversation the archive holds, a DM or a group: the
//! queueing insert reads it from `telegram_conversations`, so a request cannot
//! address anyone else, nor post to a broadcast.

use anyhow::Result;
use sqlx::MySqlPool;

/// Queue `text` for a DM or group the archive holds, answering the message
/// whose archive row is `reply_to` when given. `None` for anything else, a
/// `reply_to` outside this conversation included.
pub async fn queue(
    pool: &MySqlPool,
    conversation: &str,
    text: &str,
    reply_to: Option<i64>,
) -> Result<Option<i64>> {
    let Ok(id) = conversation.parse::<i64>() else {
        return Ok(None);
    };
    // The row is looked up in this conversation, so a reply cannot reach into
    // another; Telegram is sent the message's own `msg_id`.
    let queued = sqlx::query(
        "INSERT INTO telegram_outbox (conversation_id, text, reply_to_msg_id)
         SELECT c.id, ?, m.msg_id
           FROM telegram_conversations c
           LEFT JOIN telegram_messages m ON m.conversation_id = c.id AND m.id = ?
          WHERE c.id = ? AND c.kind IN ('dm', 'group')
            AND (? IS NULL OR m.msg_id IS NOT NULL)",
    )
    .bind(text)
    .bind(reply_to)
    .bind(id)
    .bind(reply_to)
    .execute(pool)
    .await?;
    if queued.rows_affected() != 1 {
        return Ok(None);
    }
    Ok(Some(i64::try_from(queued.last_insert_id())?))
}
