//! Telegram's writes: conversations, messages with their edits, reactions,
//! entities and calls, read marks, fetched media, and walk progress.

use anyhow::Result;
use sqlx::AssertSqlSafe;

use super::Db;

impl Db {
    pub async fn upsert_telegram_conversation(
        &self,
        id: i64,
        kind: crate::telegram::ConvKind,
        name: Option<&str>,
        username: Option<&str>,
    ) -> Result<()> {
        // Telegram routinely sends minimal peers without a title; those must not
        // blank a stored name.
        sqlx::query(
            "INSERT INTO telegram_conversations (id, kind, name, username)
             VALUES (?, ?, ?, ?)
             ON DUPLICATE KEY UPDATE
                 kind = VALUES(kind),
                 name = COALESCE(VALUES(name), name),
                 username = COALESCE(VALUES(username), username)",
        )
        .bind(id)
        .bind(kind.as_str())
        .bind(name)
        .bind(username)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Store a mapped message, keeping any text it is replacing.
    ///
    /// Telegram edits in place, so the old text is appended to
    /// `telegram_message_edits` in the same transaction as the update. An
    /// unchanged `edit_date` makes replay a no-op.
    ///
    /// No `SELECT … FOR UPDATE`: a locking read of a missing row takes an InnoDB
    /// gap lock, and the backfill and live stream then deadlock. The edit is a
    /// compare-and-swap on `edited_at <=> <value read>` instead (`<=>` because
    /// that value is NULL before the first edit); losing the race changes
    /// nothing.
    pub async fn store_telegram_message(
        &self,
        row: &crate::telegram::map::Row,
        sender_name: Option<&str>,
    ) -> Result<TelegramStored> {
        let inserted = sqlx::query(
            "INSERT IGNORE INTO telegram_messages
                (conversation_id, msg_id, sent_at, sender_id, sender_name,
                 is_outgoing, kind, text, media_kind, media_size, media_mime,
                 edited_at, edit_hidden, reply_to_msg_id, fwd_from_id, fwd_from_name,
                 fwd_date, fwd_channel_post, grouped_id, via_bot_id, ttl_period,
                 reply_quote, reply_to_peer_id, service_action)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                     ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(row.conversation_id)
        .bind(row.msg_id)
        .bind(row.sent_at)
        .bind(row.sender_id)
        .bind(sender_name)
        .bind(row.is_outgoing)
        .bind(row.kind.as_str())
        .bind(row.text.as_deref())
        .bind(row.media_kind.map(|m| m.as_str()))
        .bind(row.media_size)
        .bind(row.media_mime.as_deref())
        .bind(row.edited_at)
        .bind(row.edit_hidden)
        .bind(row.reply_to_msg_id)
        .bind(row.fwd_from_id)
        .bind(row.fwd_from_name.as_deref())
        .bind(row.fwd_date)
        .bind(row.fwd_channel_post)
        .bind(row.grouped_id)
        .bind(row.via_bot_id)
        .bind(row.ttl_period)
        .bind(row.reply_quote.as_deref())
        .bind(row.reply_to_peer_id)
        .bind(row.service_action)
        .execute(&self.pool)
        .await?;
        if inserted.rows_affected() != 0 {
            return Ok(TelegramStored::Inserted);
        }

        // Enrichment: fill columns the stored row has as NULL, so a re-walk can
        // populate columns added after the row landed. Every column that can be
        // NULL for a reason other than "the message has none" belongs here.
        // A known value is never overwritten: these facts do not change, so a
        // disagreement is a bug to see, not to paper over.
        let enriched = sqlx::query(
            "UPDATE telegram_messages
                SET media_kind = COALESCE(media_kind, ?),
                    media_size = COALESCE(media_size, ?),
                    media_mime = COALESCE(media_mime, ?),
                    edit_hidden = COALESCE(edit_hidden, ?),
                    fwd_from_id = COALESCE(fwd_from_id, ?),
                    fwd_from_name = COALESCE(fwd_from_name, ?),
                    sender_name = COALESCE(sender_name, ?),
                    fwd_date = COALESCE(fwd_date, ?),
                    fwd_channel_post = COALESCE(fwd_channel_post, ?),
                    grouped_id = COALESCE(grouped_id, ?),
                    via_bot_id = COALESCE(via_bot_id, ?),
                    ttl_period = COALESCE(ttl_period, ?),
                    reply_quote = COALESCE(reply_quote, ?),
                    reply_to_peer_id = COALESCE(reply_to_peer_id, ?),
                    service_action = COALESCE(service_action, ?)
              WHERE conversation_id = ? AND msg_id = ?
                AND ((media_size IS NULL AND ? IS NOT NULL)
                  OR (media_mime IS NULL AND ? IS NOT NULL)
                  OR (media_kind IS NULL AND ? IS NOT NULL)
                  OR (fwd_from_id IS NULL AND ? IS NOT NULL)
                  OR (fwd_from_name IS NULL AND ? IS NOT NULL)
                  OR (sender_name IS NULL AND ? IS NOT NULL)
                  OR (fwd_date IS NULL AND ? IS NOT NULL)
                  OR (fwd_channel_post IS NULL AND ? IS NOT NULL)
                  OR (grouped_id IS NULL AND ? IS NOT NULL)
                  OR (via_bot_id IS NULL AND ? IS NOT NULL)
                  OR (ttl_period IS NULL AND ? IS NOT NULL)
                  OR (reply_quote IS NULL AND ? IS NOT NULL)
                  OR (reply_to_peer_id IS NULL AND ? IS NOT NULL)
                  OR (service_action IS NULL AND ? IS NOT NULL)
                  OR edit_hidden IS NULL)",
        )
        .bind(row.media_kind.map(|m| m.as_str()))
        .bind(row.media_size)
        .bind(row.media_mime.as_deref())
        .bind(row.edit_hidden)
        .bind(row.fwd_from_id)
        .bind(row.fwd_from_name.as_deref())
        .bind(sender_name)
        .bind(row.fwd_date)
        .bind(row.fwd_channel_post)
        .bind(row.grouped_id)
        .bind(row.via_bot_id)
        .bind(row.ttl_period)
        .bind(row.reply_quote.as_deref())
        .bind(row.reply_to_peer_id)
        .bind(row.service_action)
        .bind(row.conversation_id)
        .bind(row.msg_id)
        .bind(row.media_size)
        .bind(row.media_mime.as_deref())
        .bind(row.media_kind.map(|m| m.as_str()))
        .bind(row.fwd_from_id)
        .bind(row.fwd_from_name.as_deref())
        .bind(sender_name)
        .bind(row.fwd_date)
        .bind(row.fwd_channel_post)
        .bind(row.grouped_id)
        .bind(row.via_bot_id)
        .bind(row.ttl_period)
        .bind(row.reply_quote.as_deref())
        .bind(row.reply_to_peer_id)
        .bind(row.service_action)
        .execute(&self.pool)
        .await?
        .rows_affected()
            != 0;

        let existing: Option<(Option<String>, Option<i64>)> = sqlx::query_as(
            "SELECT text, edited_at FROM telegram_messages
              WHERE conversation_id = ? AND msg_id = ?",
        )
        .bind(row.conversation_id)
        .bind(row.msg_id)
        .fetch_optional(&self.pool)
        .await?;

        let mut tx = self.pool.begin().await?;
        let outcome = match existing {
            // Gone between the insert and the read; the next delivery inserts it.
            None => TelegramStored::Unchanged,
            Some((_, stored_edit)) if stored_edit == row.edited_at => {
                if enriched {
                    TelegramStored::Enriched
                } else {
                    TelegramStored::Unchanged
                }
            }
            Some((stored_text, stored_edit)) => {
                // The superseded version, under the `edit_date` it carried; see v18.
                sqlx::query(
                    "INSERT IGNORE INTO telegram_message_edits
                        (conversation_id, msg_id, was_edited_at, text)
                     VALUES (?, ?, ?, ?)",
                )
                .bind(row.conversation_id)
                .bind(row.msg_id)
                .bind(stored_edit)
                .bind(stored_text.as_deref())
                .execute(&mut *tx)
                .await?;
                let updated = sqlx::query(
                    "UPDATE telegram_messages
                        SET text = ?, media_kind = ?, edited_at = ?
                      WHERE conversation_id = ? AND msg_id = ? AND edited_at <=> ?",
                )
                .bind(row.text.as_deref())
                .bind(row.media_kind.map(|m| m.as_str()))
                .bind(row.edited_at)
                .bind(row.conversation_id)
                .bind(row.msg_id)
                .bind(stored_edit)
                .execute(&mut *tx)
                .await?;
                if updated.rows_affected() == 0 {
                    // Another writer edited it since the read, and filed its history.
                    TelegramStored::Unchanged
                } else {
                    TelegramStored::Edited
                }
            }
        };
        tx.commit().await?;
        Ok(outcome)
    }

    /// Set a message's reaction counts to what Telegram last reported, dating any
    /// reaction missing from a non-empty report.
    ///
    /// An empty report marks nothing: Telegram omits the field both when there are
    /// no reactions and when the delivery does not carry them. So removing the
    /// last reaction goes unrecorded.
    pub async fn replace_telegram_reactions(
        &self,
        conversation_id: i64,
        msg_id: i32,
        reactions: &[crate::telegram::map::Reaction],
    ) -> Result<()> {
        if reactions.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;

        // A reaction marked removed that reappears is current again.
        for r in reactions {
            sqlx::query(
                "INSERT INTO telegram_reactions
                    (conversation_id, msg_id, emoji, custom_emoji_id, cnt, chosen)
                 VALUES (?, ?, ?, ?, ?, ?)
                 ON DUPLICATE KEY UPDATE
                    cnt = VALUES(cnt), chosen = VALUES(chosen), removed_at = NULL",
            )
            .bind(conversation_id)
            .bind(msg_id)
            .bind(r.emoji.as_deref())
            .bind(r.custom_emoji_id)
            .bind(r.cnt)
            .bind(r.chosen)
            .execute(&mut *tx)
            .await?;
        }

        // On `reaction_key`, the non-null identity: `NOT IN` over a nullable column
        // is never true.
        let keep = reactions
            .iter()
            .map(|r| reaction_key(r.emoji.as_deref(), r.custom_emoji_id))
            .collect::<Vec<_>>();
        let placeholders = vec!["?"; keep.len()].join(",");
        let sql = format!(
            "UPDATE telegram_reactions SET removed_at = CURRENT_TIMESTAMP
              WHERE conversation_id = ? AND msg_id = ? AND removed_at IS NULL
                AND reaction_key NOT IN ({placeholders})",
        );
        let mut q = sqlx::query(AssertSqlSafe(sql))
            .bind(conversation_id)
            .bind(msg_id);
        for k in &keep {
            q = q.bind(k);
        }
        q.execute(&mut *tx).await?;

        tx.commit().await?;
        Ok(())
    }

    /// Record who reacted. Retracts only when the list is
    /// [`complete`](crate::telegram::map::Reactions::complete): Telegram truncates
    /// `recent_reactions` for many reactors, and dating the unnamed would invent
    /// removals.
    pub async fn record_telegram_reaction_authors(
        &self,
        conversation_id: i64,
        msg_id: i32,
        reactions: &crate::telegram::map::Reactions,
    ) -> Result<()> {
        if reactions.authors.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;

        // `reacted_at` keeps its first observation.
        for a in &reactions.authors {
            sqlx::query(
                "INSERT INTO telegram_reaction_authors
                    (conversation_id, msg_id, peer_id, emoji, custom_emoji_id, reacted_at)
                 VALUES (?, ?, ?, ?, ?, ?)
                 ON DUPLICATE KEY UPDATE removed_at = NULL",
            )
            .bind(conversation_id)
            .bind(msg_id)
            .bind(a.peer_id)
            .bind(a.emoji.as_deref())
            .bind(a.custom_emoji_id)
            .bind(a.reacted_at)
            .execute(&mut *tx)
            .await?;
        }

        if reactions.complete {
            // On `reaction_key`, as in `replace_telegram_reactions`.
            let keep = reactions
                .authors
                .iter()
                .map(|a| {
                    (
                        a.peer_id,
                        reaction_key(a.emoji.as_deref(), a.custom_emoji_id),
                    )
                })
                .collect::<Vec<_>>();
            let placeholders = vec!["(?,?)"; keep.len()].join(",");
            let sql = format!(
                "UPDATE telegram_reaction_authors SET removed_at = CURRENT_TIMESTAMP
                  WHERE conversation_id = ? AND msg_id = ? AND removed_at IS NULL
                    AND (peer_id, reaction_key) NOT IN ({placeholders})",
            );
            let mut q = sqlx::query(AssertSqlSafe(sql))
                .bind(conversation_id)
                .bind(msg_id);
            for (peer_id, key) in &keep {
                q = q.bind(peer_id).bind(key);
            }
            q.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Record a message's entities, dating any missing from a non-empty list. An
    /// empty list marks nothing, for the reason given on
    /// [`Self::replace_telegram_reactions`].
    pub async fn replace_telegram_entities(
        &self,
        conversation_id: i64,
        msg_id: i32,
        entities: &[crate::telegram::map::Entity],
    ) -> Result<()> {
        if entities.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;

        for e in entities {
            sqlx::query(
                "INSERT INTO telegram_message_entities
                    (conversation_id, msg_id, kind, offset_utf16, length_utf16,
                     url, user_id, language, document_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON DUPLICATE KEY UPDATE
                    url = VALUES(url), user_id = VALUES(user_id),
                    language = VALUES(language), document_id = VALUES(document_id),
                    removed_at = NULL",
            )
            .bind(conversation_id)
            .bind(msg_id)
            .bind(e.kind)
            .bind(e.offset_utf16)
            .bind(e.length_utf16)
            .bind(e.url.as_deref())
            .bind(e.user_id)
            .bind(e.language.as_deref())
            .bind(e.document_id)
            .execute(&mut *tx)
            .await?;
        }

        let placeholders = vec!["(?,?,?)"; entities.len()].join(",");
        let sql = format!(
            "UPDATE telegram_message_entities SET removed_at = CURRENT_TIMESTAMP
              WHERE conversation_id = ? AND msg_id = ? AND removed_at IS NULL
                AND (kind, offset_utf16, length_utf16) NOT IN ({placeholders})",
        );
        let mut q = sqlx::query(AssertSqlSafe(sql))
            .bind(conversation_id)
            .bind(msg_id);
        for e in entities {
            q = q.bind(e.kind).bind(e.offset_utf16).bind(e.length_utf16);
        }
        q.execute(&mut *tx).await?;

        tx.commit().await?;
        Ok(())
    }

    /// Record how long a call was and how it ended. The service message exists
    /// from the call's start, so an early delivery must not erase a duration a
    /// later one learned.
    pub async fn record_telegram_call(
        &self,
        conversation_id: i64,
        msg_id: i32,
        call: &crate::telegram::map::Call,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO telegram_calls
                (conversation_id, msg_id, call_id, duration_s, reason, video)
             VALUES (?, ?, ?, ?, ?, ?)
             ON DUPLICATE KEY UPDATE
                duration_s = COALESCE(duration_s, VALUES(duration_s)),
                reason = COALESCE(reason, VALUES(reason)),
                video = VALUES(video)",
        )
        .bind(conversation_id)
        .bind(msg_id)
        .bind(call.call_id)
        .bind(call.duration_s)
        .bind(call.reason)
        .bind(call.video)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The next batch of stored message ids to re-read, oldest first; empty when
    /// the conversation is done.
    pub async fn telegram_recapture_batch(
        &self,
        conversation_id: i64,
        limit: u32,
    ) -> Result<Vec<i32>> {
        Ok(sqlx::query_scalar(
            "SELECT m.msg_id FROM telegram_messages m
               WHERE m.conversation_id = ?
                 AND m.msg_id > COALESCE(
                     (SELECT s.through_msg_id FROM telegram_recapture_state s
                       WHERE s.conversation_id = m.conversation_id), 0)
               ORDER BY m.msg_id
               LIMIT ?",
        )
        .bind(conversation_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Move the re-capture frontier, after the batch is written. It never moves
    /// backwards.
    pub async fn record_telegram_recapture(
        &self,
        conversation_id: i64,
        through_msg_id: i32,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO telegram_recapture_state (conversation_id, through_msg_id)
             VALUES (?, ?)
             ON DUPLICATE KEY UPDATE
                through_msg_id = GREATEST(through_msg_id, VALUES(through_msg_id))",
        )
        .bind(conversation_id)
        .bind(through_msg_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// How many messages this conversation holds.
    pub async fn telegram_message_count(&self, conversation_id: i64) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT COUNT(*) FROM telegram_messages WHERE conversation_id = ?")
                .bind(conversation_id)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    /// Record a read mark, keeping its first observation's time. Returns whether
    /// it was new. A `max_id` of 0 is Telegram saying nothing has been read.
    pub async fn record_telegram_read_mark(
        &self,
        conversation_id: i64,
        direction: TelegramReadDirection,
        max_id: i32,
    ) -> Result<bool> {
        if max_id <= 0 {
            return Ok(false);
        }
        let done = sqlx::query(
            "INSERT IGNORE INTO telegram_read_marks (conversation_id, direction, max_id)
             VALUES (?, ?, ?)",
        )
        .bind(conversation_id)
        .bind(direction.as_str())
        .bind(max_id)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() != 0)
    }

    /// The furthest mark the archive holds for one conversation and direction, and
    /// when it was first seen. `None` when nothing has been read.
    pub async fn telegram_read_mark(
        &self,
        conversation_id: i64,
        direction: TelegramReadDirection,
    ) -> Result<Option<(i32, i64)>> {
        let row: Option<(i32, i64)> = sqlx::query_as(
            "SELECT max_id, UNIX_TIMESTAMP(observed_at) FROM telegram_read_marks
              WHERE conversation_id = ? AND direction = ?
              ORDER BY max_id DESC LIMIT 1",
        )
        .bind(conversation_id)
        .bind(direction.as_str())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Whether this message's bytes are already accounted for (stored, offered or
    /// failed), so the eager pass can skip it.
    pub async fn telegram_media_state(
        &self,
        conversation_id: i64,
        msg_id: i32,
    ) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT state FROM telegram_media WHERE conversation_id = ? AND msg_id = ?",
        )
        .bind(conversation_id)
        .bind(msg_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Record bytes written to the volume. Call after the download returns: the
    /// row tells the reader the file is complete.
    pub async fn record_telegram_media_stored(
        &self,
        conversation_id: i64,
        msg_id: i32,
        stored_name: &str,
        content_type: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO telegram_media
                (conversation_id, msg_id, state, stored_name, content_type, stored_at)
             VALUES (?, ?, 'stored', ?, ?, CURRENT_TIMESTAMP)
             ON DUPLICATE KEY UPDATE
                 state = 'stored', stored_name = VALUES(stored_name),
                 content_type = VALUES(content_type),
                 note = NULL, stored_at = CURRENT_TIMESTAMP",
        )
        .bind(conversation_id)
        .bind(msg_id)
        .bind(stored_name)
        .bind(content_type)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record that this message's bytes are available but not fetched, or that
    /// fetching them failed and why.
    ///
    /// Never downgrades `stored`: a re-walk offers everything it sees.
    pub async fn record_telegram_media_state(
        &self,
        conversation_id: i64,
        msg_id: i32,
        state: TelegramMediaState,
        note: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO telegram_media (conversation_id, msg_id, state, note)
             VALUES (?, ?, ?, ?)
             ON DUPLICATE KEY UPDATE
                 state = IF(state = 'stored', 'stored', VALUES(state)),
                 note = IF(state = 'stored', note, VALUES(note))",
        )
        .bind(conversation_id)
        .bind(msg_id)
        .bind(state.as_str())
        .bind(note)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// A reader asked for these bytes. Only `offered` or `failed` rows are queued,
    /// so a stored file is not re-fetched and a second tap does not reset the
    /// clock. Returns whether anything was queued.
    pub async fn request_telegram_media(&self, row_id: i64) -> Result<bool> {
        let changed = sqlx::query(
            "UPDATE telegram_media d
               JOIN telegram_messages m
                 ON m.conversation_id = d.conversation_id AND m.msg_id = d.msg_id
                SET d.state = 'wanted', d.requested_at = CURRENT_TIMESTAMP
              WHERE m.id = ? AND d.state IN ('offered', 'failed')",
        )
        .bind(row_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(changed != 0)
    }

    /// What readers have asked for, oldest request first.
    pub async fn wanted_telegram_media(&self, limit: i64) -> Result<Vec<(i64, i32)>> {
        Ok(sqlx::query_as(
            "SELECT conversation_id, msg_id FROM telegram_media
              WHERE state = 'wanted' ORDER BY requested_at ASC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    /// How far back a conversation has been walked, or `None` if it never has.
    pub async fn telegram_backfill_state(
        &self,
        conversation_id: i64,
    ) -> Result<Option<TelegramBackfill>> {
        let row: Option<(Option<i32>, i8, i64)> = sqlx::query_as(
            "SELECT oldest_seen, complete, messages_stored
               FROM telegram_backfill_state WHERE conversation_id = ?",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(oldest_seen, complete, messages_stored)| TelegramBackfill {
                oldest_seen,
                complete: complete != 0,
                messages_stored,
            },
        ))
    }

    /// Record progress through a conversation's history. `LEAST` because the live
    /// stream reports high ids through the same path, which must not reset the
    /// frontier.
    pub async fn record_telegram_backfill(
        &self,
        conversation_id: i64,
        oldest_seen: Option<i32>,
        complete: bool,
        stored_delta: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO telegram_backfill_state
                (conversation_id, oldest_seen, complete, messages_stored)
             VALUES (?, ?, ?, ?)
             ON DUPLICATE KEY UPDATE
                 oldest_seen = LEAST(COALESCE(VALUES(oldest_seen), oldest_seen),
                                     COALESCE(oldest_seen, VALUES(oldest_seen))),
                 complete = GREATEST(complete, VALUES(complete)),
                 messages_stored = messages_stored + VALUES(messages_stored)",
        )
        .bind(conversation_id)
        .bind(oldest_seen)
        .bind(complete)
        .bind(stored_delta)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Flag deleted messages, keeping their text, and return how many rows matched.
    ///
    /// `updateDeleteMessages` names no peer: private chats and basic groups share
    /// one id sequence per account. Channels have their own sequences and
    /// `updateDeleteChannelMessages`, so a peer-less deletion skips channels.
    pub async fn mark_telegram_deleted(
        &self,
        msg_ids: &[i32],
        scope: TelegramDeleteScope,
    ) -> Result<u64> {
        if msg_ids.is_empty() {
            return Ok(0);
        }
        let mut affected = 0;
        for id in msg_ids {
            let res = match scope {
                TelegramDeleteScope::Channel(conversation_id) => {
                    sqlx::query(
                        "UPDATE telegram_messages
                        SET deleted = 1, deleted_at = COALESCE(deleted_at, CURRENT_TIMESTAMP)
                      WHERE conversation_id = ? AND msg_id = ?",
                    )
                    .bind(conversation_id)
                    .bind(id)
                    .execute(&self.pool)
                    .await?
                }
                TelegramDeleteScope::SharedSequence => {
                    sqlx::query(
                        "UPDATE telegram_messages m
                       JOIN telegram_conversations c ON c.id = m.conversation_id
                        SET m.deleted = 1,
                            m.deleted_at = COALESCE(m.deleted_at, CURRENT_TIMESTAMP)
                      WHERE m.msg_id = ? AND c.kind <> 'channel'",
                    )
                    .bind(id)
                    .execute(&self.pool)
                    .await?
                }
            };
            affected += res.rows_affected();
        }
        Ok(affected)
    }
}

/// A reaction's `reaction_key`, as the generated column computes it (v17).
fn reaction_key(emoji: Option<&str>, custom_emoji_id: Option<i64>) -> String {
    match (emoji, custom_emoji_id) {
        (Some(e), _) => e.to_string(),
        (None, Some(id)) => format!("custom:{id}"),
        (None, None) => String::new(),
    }
}

/// What [`Db::store_telegram_message`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramStored {
    /// A message the archive had never seen.
    Inserted,
    /// A message whose text was replaced, with the old version kept.
    Edited,
    /// Already stored, and this delivery filled a NULL column.
    Enriched,
    /// Already stored, and nothing new.
    Unchanged,
}

/// How far back through a conversation the walk has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelegramBackfill {
    /// The lowest `msg_id` stored so far, or `None` before the first page.
    pub oldest_seen: Option<i32>,
    /// Set once a page came back empty, which is Telegram's only signal that a
    /// conversation has no more history.
    pub complete: bool,
    pub messages_stored: i64,
}

/// Whose reading a mark describes, in Telegram's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramReadDirection {
    /// How far I have read their messages.
    Inbox,
    /// How far they have read mine.
    Outbox,
}

impl TelegramReadDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            TelegramReadDirection::Inbox => "inbox",
            TelegramReadDirection::Outbox => "outbox",
        }
    }
}

/// What the archive holds, or does not, for one message's media.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramMediaState {
    /// Telegram has the bytes and we have not fetched them.
    Offered,
    /// A reader asked for it and the feed has not got to it yet.
    Wanted,
    /// On the volume.
    Stored,
    /// Tried and failed; `note` says why.
    Failed,
}

impl TelegramMediaState {
    pub fn as_str(self) -> &'static str {
        match self {
            TelegramMediaState::Offered => "offered",
            TelegramMediaState::Wanted => "wanted",
            TelegramMediaState::Stored => "stored",
            TelegramMediaState::Failed => "failed",
        }
    }
}

/// Which conversations a deletion applies to; see [`Db::mark_telegram_deleted`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramDeleteScope {
    /// `updateDeleteChannelMessages`, which names its channel.
    Channel(i64),
    /// `updateDeleteMessages`, whose ids are in the sequence private chats and
    /// basic groups share.
    SharedSequence,
}
