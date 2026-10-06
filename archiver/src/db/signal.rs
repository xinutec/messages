//! Signal's writes: conversations, contacts and their names, messages with their
//! edits, styles, previews, attachments and reactions, receipts, calls, and the
//! raw frames everything is parsed from.

use anyhow::Result;

use super::Db;
use crate::parse::ThreadId;

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
    pub async fn set_conversation_name(&self, thread_id: &str, name: &str) -> Result<()> {
        if name.is_empty() {
            return Ok(());
        }
        sqlx::query("UPDATE conversations SET name = ? WHERE thread_id = ?")
            .bind(name)
            .bind(thread_id)
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
        // The duplicate branch leaves the name alone, so a rename happens only below.
        // `rows_affected` is 1 exactly when this inserted.
        let inserted = sqlx::query(
            "INSERT INTO contacts (uuid, phone, display_name) VALUES (?, ?, ?)
             ON DUPLICATE KEY UPDATE phone = COALESCE(VALUES(phone), phone)",
        )
        .bind(uuid)
        .bind(phone)
        .bind(name)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;

        let Some(name) = name else { return Ok(()) };

        // `IS NULL`: a contact first seen nameless gets its first name here.
        let moved = sqlx::query(
            "UPDATE contacts SET display_name = ?
              WHERE uuid = ? AND (display_name IS NULL OR display_name <> ?)",
        )
        .bind(name)
        .bind(uuid)
        .bind(name)
        .execute(&self.pool)
        .await?
        .rows_affected();

        if !inserted && moved == 0 {
            return Ok(());
        }
        self.record_contact_name(uuid, name).await
    }

    /// Close the previous name and open the current one.
    ///
    /// Separate statements because MariaDB refuses an `INSERT … WHERE NOT EXISTS`
    /// whose subquery reads the target table.
    async fn record_contact_name(&self, uuid: &str, name: &str) -> Result<()> {
        sqlx::query(
            "UPDATE contact_names SET seen_until = CURRENT_TIMESTAMP
              WHERE uuid = ? AND seen_until IS NULL AND name <> ?",
        )
        .bind(uuid)
        .bind(name)
        .execute(&self.pool)
        .await?;
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM contact_names WHERE uuid = ? AND seen_until IS NULL",
        )
        .bind(uuid)
        .fetch_one(&self.pool)
        .await?;
        if open == 0 {
            sqlx::query("INSERT INTO contact_names (uuid, name) VALUES (?, ?)")
                .bind(uuid)
                .bind(name)
                .execute(&self.pool)
                .await?;
        }
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
                 quote_author_uuid, quote_text)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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

    /// Flag an archived original as edited (content kept; edits are separate rows).
    /// Returns rows marked (0 if we never archived the original).
    pub async fn mark_edited(&self, sender_uuid: &str, target_ts: i64) -> Result<u64> {
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

    /// Store an edited version as its own row, linked to the original via edit_of_ts.
    pub async fn insert_edit(
        &self,
        thread_id: &ThreadId,
        sender_uuid: &str,
        edit_ts: i64,
        body: Option<&str>,
        edit_of_ts: i64,
        is_outgoing: bool,
    ) -> Result<Option<u64>> {
        let res = sqlx::query(
            "INSERT IGNORE INTO messages \
                (thread_id, sender_uuid, server_ts, body, is_outgoing, edit_of_ts) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(thread_id.to_string())
        .bind(sender_uuid)
        .bind(edit_ts)
        .bind(body)
        .bind(is_outgoing)
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
        .bind(position as i32)
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

    pub async fn insert_attachment(
        &self,
        message_id: u64,
        content_type: Option<&str>,
        file_name: Option<&str>,
        size_bytes: Option<i64>,
        stored_path: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO attachments (message_id, content_type, file_name, size_bytes, stored_path)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(message_id)
        .bind(content_type)
        .bind(file_name)
        .bind(size_bytes)
        .bind(stored_path)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_reaction(
        &self,
        thread_id: &ThreadId,
        target_ts: i64,
        author_uuid: &str,
        emoji: Option<&str>,
        reaction_ts: i64,
        removed: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT IGNORE INTO reactions
                (thread_id, target_ts, author_uuid, emoji, reaction_ts, removed)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(thread_id.to_string())
        .bind(target_ts)
        .bind(author_uuid)
        .bind(emoji)
        .bind(reaction_ts)
        .bind(removed)
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
}
