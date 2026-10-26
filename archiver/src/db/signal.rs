//! Signal's writes: conversations, contacts and their names, messages with their
//! edits, styles, previews, attachments and reactions, receipts, calls, and the
//! raw frames everything is parsed from.

use anyhow::Result;
use sqlx::{Connection, MySqlConnection};

use super::Db;
use crate::parse::ThreadId;

/// Close the previous name and open the current one, on the caller's
/// transaction.
///
/// Separate statements because MariaDB refuses an `INSERT … WHERE NOT EXISTS`
/// whose subquery reads the target table.
async fn record_contact_name(conn: &mut MySqlConnection, uuid: &str, name: &str) -> Result<()> {
    sqlx::query(
        "UPDATE contact_names SET seen_until = CURRENT_TIMESTAMP
          WHERE uuid = ? AND seen_until IS NULL AND name <> ?",
    )
    .bind(uuid)
    .bind(name)
    .execute(&mut *conn)
    .await?;
    let open: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM contact_names WHERE uuid = ? AND seen_until IS NULL",
    )
    .bind(uuid)
    .fetch_one(&mut *conn)
    .await?;
    if open == 0 {
        sqlx::query("INSERT INTO contact_names (uuid, name) VALUES (?, ?)")
            .bind(uuid)
            .bind(name)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

impl Db {
    pub async fn upsert_conversation(&self, thread: &ThreadId) -> Result<()> {
        sqlx::query(
            "INSERT INTO conversations (thread_id, type) VALUES (?, ?)
             ON DUPLICATE KEY UPDATE updated_at = CURRENT_TIMESTAMP",
        )
        .bind(thread.to_string())
        .bind(thread.kind().as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set a conversation's display name (DM contact name or group title). No-op
    /// for an empty name.
    pub async fn set_conversation_name(&self, thread_id: &ThreadId, name: &str) -> Result<()> {
        if name.is_empty() {
            return Ok(());
        }
        sqlx::query("UPDATE conversations SET name = ? WHERE thread_id = ?")
            .bind(name)
            .bind(thread_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record a contact, dating the name it wore when it is renamed.
    ///
    /// The name is written by a second statement whose `rows_affected` is the
    /// rename: a name arrives with every message, almost always unchanged, and
    /// the `<>` makes that a no-op. A sighting without a name (receipts, typing,
    /// unknown group members) never blanks one we hold.
    ///
    /// `name` is `envelope.sourceName`, a display name; see migration v41.
    pub async fn upsert_contact(
        &self,
        uuid: &str,
        phone: Option<&str>,
        name: Option<&str>,
    ) -> Result<()> {
        let phone = phone.filter(|s| !s.is_empty());
        let name = name.filter(|s| !s.is_empty());
        // One transaction: a rename and its history row land together, or
        // neither does. READ COMMITTED, so closing a previous name that does not
        // exist locks no gap: under REPEATABLE READ two first sightings deadlock
        // on each other's gap when they insert.
        let mut conn = self.pool.acquire().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
            .execute(&mut *conn)
            .await?;
        let mut tx = conn.begin().await?;
        sqlx::query(
            "INSERT INTO contacts (uuid, phone) VALUES (?, ?)
             ON DUPLICATE KEY UPDATE phone = COALESCE(VALUES(phone), phone)",
        )
        .bind(uuid)
        .bind(phone)
        .execute(&mut *tx)
        .await?;

        if let Some(name) = name {
            // The row's name is set here, never by the INSERT, so one count says
            // the name is new for this contact: its first, or a rename.
            // `IS NULL`: a contact first seen nameless gets its first name here.
            let moved = sqlx::query(
                "UPDATE contacts SET display_name = ?
                  WHERE uuid = ? AND (display_name IS NULL OR display_name <> ?)",
            )
            .bind(name)
            .bind(uuid)
            .bind(name)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if moved > 0 {
                record_contact_name(&mut tx, uuid, name).await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Keep the frame exactly as it arrived, before parsing. Callers treat a
    /// failure here as non-fatal: losing the row beats losing the message.
    ///
    /// Returns whether the frame was new; a re-delivery hashes the same and is
    /// ignored.
    pub async fn record_signal_frame(&self, frame: &serde_json::Value) -> Result<bool> {
        use sha2::{Digest, Sha256};
        // The same bytes are hashed and stored.
        let bytes = serde_json::to_vec(frame)?;
        let digest = Sha256::digest(&bytes);
        let env = frame
            .get("envelope")
            .or_else(|| frame.get("params").and_then(|p| p.get("envelope")));
        let envelope_ts = env
            .and_then(|e| e.get("timestamp"))
            .and_then(|t| t.as_i64());
        let source_uuid = env
            .and_then(|e| e.get("sourceUuid"))
            .and_then(|s| s.as_str());
        let n = sqlx::query(
            "INSERT IGNORE INTO signal_frames (digest, envelope_ts, source_uuid, frame)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&digest[..])
        .bind(envelope_ts)
        .bind(source_uuid)
        .bind(&bytes)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n > 0)
    }

    /// Insert a message, returning its row id, or `None` for a duplicate.
    pub async fn insert_message(&self, m: &crate::parse::Message) -> Result<Option<u64>> {
        let res = sqlx::query(
            "INSERT IGNORE INTO messages
                (thread_id, sender_uuid, server_ts, body, quote_target_ts, is_outgoing,
                 server_received_ts, server_delivered_ts, expires_in_seconds,
                 quote_author_uuid, quote_text, group_call_era)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(m.thread_id.to_string())
        .bind(&m.sender)
        .bind(m.server_ts)
        .bind(m.body.as_deref())
        .bind(m.quote_target_ts)
        .bind(m.is_outgoing)
        .bind(m.server_received_ts)
        .bind(m.server_delivered_ts)
        .bind(m.expires_in_seconds)
        .bind(m.quote_author.as_deref())
        .bind(m.quote_text.as_deref())
        .bind(m.group_call.as_deref())
        .execute(&self.pool)
        .await?;
        Ok((res.rows_affected() != 0).then(|| res.last_insert_id()))
    }

    /// Flag an archived message as deleted-for-everyone (content is kept).
    /// Returns the number of rows marked (0 if we never archived the original).
    pub async fn mark_deleted(&self, sender_uuid: &str, target_ts: i64) -> Result<u64> {
        let res = sqlx::query(
            "UPDATE messages SET deleted = 1, deleted_at = CURRENT_TIMESTAMP \
             WHERE sender_uuid = ? AND server_ts = ? AND deleted = 0",
        )
        .bind(sender_uuid)
        .bind(target_ts)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// The original an edit's target stands for: Signal can address an edit to
    /// the edit before it, and every version is filed against the original, as
    /// the viewer gathers them. The target itself when it is no edit, or unknown.
    async fn edit_root(&self, sender_uuid: &str, target_ts: i64) -> Result<i64> {
        let root: Option<i64> = sqlx::query_scalar(
            "SELECT edit_of_ts FROM messages
              WHERE sender_uuid = ? AND server_ts = ? AND edit_of_ts IS NOT NULL
              LIMIT 1",
        )
        .bind(sender_uuid)
        .bind(target_ts)
        .fetch_optional(&self.pool)
        .await?;
        Ok(root.unwrap_or(target_ts))
    }

    /// Flag an archived original as edited (content kept; edits are separate rows).
    /// Returns rows marked (0 if we never archived the original).
    pub async fn mark_edited(&self, sender_uuid: &str, target_ts: i64) -> Result<u64> {
        let target_ts = self.edit_root(sender_uuid, target_ts).await?;
        let res = sqlx::query(
            "UPDATE messages SET edited = 1 \
             WHERE sender_uuid = ? AND server_ts = ? AND edit_of_ts IS NULL",
        )
        .bind(sender_uuid)
        .bind(target_ts)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// Store an edited version as its own row, linked to the original via
    /// `edit_of_ts`, also when Signal named an earlier edit (`edit_root`).
    pub async fn insert_edit(&self, e: &crate::parse::Edit) -> Result<Option<u64>> {
        let edit_of_ts = self.edit_root(&e.sender, e.target_ts).await?;
        let res = sqlx::query(
            "INSERT IGNORE INTO messages \
                (thread_id, sender_uuid, server_ts, body, is_outgoing, edit_of_ts) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(e.thread_id.to_string())
        .bind(&e.sender)
        .bind(e.edit_ts)
        .bind(e.body.as_deref())
        .bind(e.is_outgoing)
        .bind(edit_of_ts)
        .execute(&self.pool)
        .await?;
        Ok((res.rows_affected() != 0).then(|| res.last_insert_id()))
    }

    /// Record a message row's styled runs.
    pub async fn insert_text_styles(
        &self,
        message_id: u64,
        styles: &[crate::parse::TextStyle],
    ) -> Result<()> {
        for st in styles {
            sqlx::query(
                "INSERT IGNORE INTO signal_text_styles
                    (message_id, style, start_utf16, length_utf16)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(message_id)
            .bind(&st.style)
            .bind(st.start_utf16)
            .bind(st.length_utf16)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Record who a message row's body names, and where.
    pub async fn insert_mentions(
        &self,
        message_id: u64,
        mentions: &[crate::parse::Mention],
    ) -> Result<()> {
        for x in mentions {
            sqlx::query(
                "INSERT IGNORE INTO signal_mentions (message_id, start_utf16, length_utf16, uuid)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(message_id)
            .bind(x.start_utf16)
            .bind(x.length_utf16)
            .bind(&x.uuid)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Record one of a message row's link previews; `position` is its place in
    /// the order sent, and `image_path` where its picture was stored, if it was.
    pub async fn insert_link_preview(
        &self,
        message_id: u64,
        position: usize,
        p: &crate::parse::LinkPreview,
        image_path: Option<&str>,
    ) -> Result<()> {
        let image = p.image.as_ref();
        sqlx::query(
            "INSERT IGNORE INTO signal_link_previews
                (message_id, position, url, title, description,
                 image_id, image_content_type, image_path)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(message_id)
        .bind(i32::try_from(position)?)
        .bind(&p.url)
        .bind(p.title.as_deref())
        .bind(p.description.as_deref())
        .bind(image.and_then(|i| i.id.as_deref()))
        .bind(image.and_then(|i| i.content_type.as_deref()))
        .bind(image_path)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Previews whose picture is named but not stored: (preview id, attachment id).
    pub async fn preview_images_to_fetch(&self) -> Result<Vec<(i64, String)>> {
        Ok(sqlx::query_as(
            "SELECT id, image_id FROM signal_link_previews
              WHERE image_id IS NOT NULL AND image_path IS NULL",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn set_preview_image_path(&self, id: i64, path: &str) -> Result<()> {
        sqlx::query("UPDATE signal_link_previews SET image_path = ? WHERE id = ?")
            .bind(path)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record one of a message row's attachments, and where its bytes were
    /// stored, if they were.
    pub async fn insert_attachment(
        &self,
        message_id: u64,
        a: &crate::parse::Attachment,
        stored_path: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO attachments (message_id, content_type, file_name, size_bytes, stored_path)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(message_id)
        .bind(a.content_type.as_deref())
        .bind(a.file_name.as_deref())
        .bind(a.size)
        .bind(stored_path)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_reaction(&self, r: &crate::parse::Reaction) -> Result<()> {
        sqlx::query(
            "INSERT IGNORE INTO reactions
                (thread_id, target_ts, author_uuid, emoji, reaction_ts, removed)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(r.thread_id.to_string())
        .bind(r.target_ts)
        .bind(&r.author)
        .bind(r.emoji.as_deref())
        .bind(r.reaction_ts)
        .bind(r.removed)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record a receipt, keeping the first observation's time. Returns how many
    /// rows were new.
    pub async fn record_signal_receipt(&self, receipt: &crate::parse::Receipt) -> Result<u64> {
        let mut written = 0;
        for target in &receipt.targets {
            written += sqlx::query(
                "INSERT IGNORE INTO signal_receipts
                    (target_ts, author_uuid, kind, when_ts)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(target)
            .bind(&receipt.author)
            .bind(receipt.kind.as_str())
            .bind(receipt.when_ts)
            .execute(&self.pool)
            .await?
            .rows_affected();
        }
        Ok(written)
    }

    /// Record one frame of a call's signalling. `event_ts` is in the key because
    /// a call can carry two frames of one kind, such as a hangup per device.
    pub async fn record_signal_call_event(&self, call: &crate::parse::CallEvent) -> Result<u64> {
        Ok(sqlx::query(
            "INSERT IGNORE INTO signal_call_events
                (call_id, peer_uuid, event, detail, device_id, event_ts)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(call.call_id)
        .bind(&call.peer)
        .bind(call.event.as_str())
        .bind(call.detail.as_deref())
        .bind(call.device_id)
        .bind(call.event_ts)
        .execute(&self.pool)
        .await?
        .rows_affected())
    }
    /// Take the oldest queued Signal send, marking it `sending` first, so the send
    /// that follows happens at most once. One queued longer ago than
    /// `fresh_secs` is failed instead: the viewer that asked has stopped waiting.
    /// A quote is read from the archive here: Signal wants its author and text.
    pub async fn claim_signal_send(&self, fresh_secs: i64) -> Result<Option<SignalSend>> {
        sqlx::query(
            "UPDATE signal_outbox SET state = 'failed', done_at = CURRENT_TIMESTAMP,
                    note = 'expired before the ingester reached it'
              WHERE state = 'queued' AND queued_at < CURRENT_TIMESTAMP - INTERVAL ? SECOND",
        )
        .bind(fresh_secs)
        .execute(&self.pool)
        .await?;
        let next: Option<QueuedSignalSend> = sqlx::query_as(
            "SELECT o.id, o.thread_id, o.text, o.quote_ts, q.sender_uuid, q.body
                   FROM signal_outbox o
                   LEFT JOIN messages q
                          ON q.thread_id = o.thread_id AND q.server_ts = o.quote_ts
                         AND q.edit_of_ts IS NULL
                  WHERE o.state = 'queued' ORDER BY o.queued_at, o.id LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, thread, text, quote_ts, author, quoted)) = next else {
            return Ok(None);
        };
        // Conditional, so a viewer withdrawing it in between wins.
        let claimed = sqlx::query(
            "UPDATE signal_outbox SET state = 'sending' WHERE id = ? AND state = 'queued'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if claimed != 1 {
            return Ok(None);
        }
        let Some(thread) = ThreadId::parse(&thread) else {
            self.record_signal_send_failed(id, "not a Signal conversation")
                .await?;
            return Ok(None);
        };
        let quote = match (quote_ts, author) {
            (None, _) => None,
            (Some(ts), Some(author)) => Some(SignalQuote {
                ts,
                author,
                text: quoted,
            }),
            (Some(_), None) => {
                self.record_signal_send_failed(id, "the message it answers is not in the archive")
                    .await?;
                return Ok(None);
            }
        };
        Ok(Some(SignalSend {
            id,
            thread,
            text,
            quote,
        }))
    }

    /// The send went out at Signal's `sent_ts`; `archived` says whether its row
    /// was written, and `note` who it did not reach.
    pub async fn record_signal_sent(
        &self,
        id: i64,
        sent_ts: i64,
        archived: bool,
        note: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE signal_outbox SET state = 'sent', sent_ts = ?, archived = ?, note = ?,
                    done_at = CURRENT_TIMESTAMP
              WHERE id = ? AND state = 'sending'",
        )
        .bind(sent_ts)
        .bind(archived)
        .bind(note.map(fit_note))
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The send did not go out, or may not have.
    pub async fn record_signal_send_failed(&self, id: i64, note: &str) -> Result<()> {
        sqlx::query(
            "UPDATE signal_outbox SET state = 'failed', note = ?, done_at = CURRENT_TIMESTAMP
              WHERE id = ? AND state = 'sending'",
        )
        .bind(fit_note(note))
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Sends a previous run claimed and never finished: whether Signal has them
    /// is unknown, so they are failed with that said, never retried.
    pub async fn abandon_interrupted_signal_sends(&self) -> Result<u64> {
        Ok(sqlx::query(
            "UPDATE signal_outbox SET state = 'failed', done_at = CURRENT_TIMESTAMP,
                    note = 'interrupted mid-send: it may or may not have gone; check Signal'
              WHERE state = 'sending'",
        )
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    /// This account's own uuid, as the sender of what it sent from the phone.
    /// `None` until one such message is archived.
    pub async fn own_uuid(&self) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT sender_uuid FROM messages WHERE is_outgoing = 1 ORDER BY id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Mark row `id` as sent by this app; see migration v65.
    pub async fn mark_sent_here(&self, id: u64) -> Result<()> {
        sqlx::query("UPDATE messages SET sent_here = 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// An outbox row as `claim_signal_send` reads it: id, thread, text, the quoted
/// message's `server_ts`, and that message's author and text when archived.
type QueuedSignalSend = (
    i64,
    String,
    String,
    Option<i64>,
    Option<String>,
    Option<String>,
);

/// A send the ingester has claimed (`Db::claim_signal_send`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalSend {
    /// The `signal_outbox` row.
    pub id: i64,
    /// Where it goes: a thread the archive holds, never the reader's word.
    pub thread: ThreadId,
    pub text: String,
    pub quote: Option<SignalQuote>,
}

/// The message a send answers, as Signal's quote names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalQuote {
    pub ts: i64,
    pub author: String,
    pub text: Option<String>,
}

fn fit_note(note: &str) -> String {
    note.chars().take(255).collect()
}
