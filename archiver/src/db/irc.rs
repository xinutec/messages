//! IRC's writes: conversations, log lines, and what the importer has read.

use std::collections::HashMap;

use anyhow::Result;
use irclog::IrcLine;
use sqlx::Row;

use super::{Db, INSERT_CHUNK};

impl Db {
    /// Ensure the conversation exists and return its id. `LAST_INSERT_ID(id)`
    /// makes the duplicate branch return the existing row's id.
    pub async fn upsert_irc_conversation(
        &self,
        network: &str,
        target: &str,
        is_channel: bool,
        is_status: bool,
    ) -> Result<u64> {
        let res = sqlx::query(
            "INSERT INTO irc_conversations (network, target, is_channel, is_status)
             VALUES (?, ?, ?, ?)
             ON DUPLICATE KEY UPDATE id = LAST_INSERT_ID(id), is_status = ?",
        )
        .bind(network)
        .bind(target)
        .bind(is_channel)
        .bind(is_status)
        .bind(is_status)
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_id())
    }

    /// Insert a log file's lines, returning how many were new.
    ///
    /// One statement per [`INSERT_CHUNK`] lines rather than per line: a history
    /// import runs over a port-forward, where each round trip is expensive.
    pub async fn insert_irc_lines(
        &self,
        conversation_id: u64,
        source_tag: &str,
        file_date: &str,
        lines: &[IrcLine],
    ) -> Result<u64> {
        let mut written = 0;
        for chunk in lines.chunks(INSERT_CHUNK) {
            let mut qb = sqlx::QueryBuilder::new(
                "INSERT IGNORE INTO irc_messages
                    (conversation_id, source_tag, file_date, line_no, sent_at, nick, is_self, kind, text) ",
            );
            qb.push_values(chunk, |mut row, line| {
                row.push_bind(conversation_id)
                    .push_bind(source_tag)
                    .push_bind(file_date)
                    .push_bind(line.line_no)
                    .push_bind(&line.sent_at)
                    .push_bind(&line.nick)
                    .push_bind(line.is_self)
                    .push_bind(line.kind.as_str())
                    .push_bind(&line.text);
            });
            written += qb.build().execute(&self.pool).await?.rows_affected();
        }
        Ok(written)
    }

    /// Every file the importer has already read, as `rel_path → (mtime_ns, size)`,
    /// read whole to save a round trip per file.
    pub async fn irc_import_state(&self) -> Result<HashMap<String, (i64, i64)>> {
        let rows = sqlx::query("SELECT rel_path, mtime_ns, size_bytes FROM irc_import_state")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|r| {
                Ok((
                    r.try_get("rel_path")?,
                    (r.try_get("mtime_ns")?, r.try_get("size_bytes")?),
                ))
            })
            .collect()
    }

    /// Mark files as imported at the state they were read in.
    ///
    /// Call only after their lines are in, and only under `--apply`: the next run
    /// skips whatever is marked. A run that dies before a flush leaves files
    /// unmarked, which is safe.
    pub async fn record_irc_imports(&self, files: &[(String, i64, i64)]) -> Result<()> {
        for chunk in files.chunks(INSERT_CHUNK) {
            let mut qb = sqlx::QueryBuilder::new(
                "INSERT INTO irc_import_state (rel_path, mtime_ns, size_bytes) ",
            );
            qb.push_values(chunk, |mut row, (rel, mtime, size)| {
                row.push_bind(rel).push_bind(mtime).push_bind(size);
            });
            qb.push(
                " ON DUPLICATE KEY UPDATE mtime_ns = VALUES(mtime_ns), size_bytes = VALUES(size_bytes)",
            );
            qb.build().execute(&self.pool).await?;
        }
        Ok(())
    }
}

/// `irc_conversations` ids already looked up, so a run upserts each once.
#[derive(Default)]
pub struct IrcConversations(std::collections::BTreeMap<(String, String), u64>);

impl IrcConversations {
    /// The conversation's id, creating it on first sight. `is_status` marks
    /// irssi's server-notice window; see migration v7.
    pub async fn id(
        &mut self,
        db: &Db,
        network: &str,
        target: &str,
        is_status: bool,
    ) -> Result<u64> {
        let key = (network.to_string(), target.to_string());
        if let Some(id) = self.0.get(&key) {
            return Ok(*id);
        }
        let id = db
            .upsert_irc_conversation(network, target, irclog::is_channel(target), is_status)
            .await?;
        self.0.insert(key, id);
        Ok(id)
    }
}
