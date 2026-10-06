//! Google Chat: `gchat_*`, a one-off import with reactions and attachments.

use std::collections::HashMap;

use anyhow::Result;
use sqlx::{AssertSqlSafe, MySqlPool, Row};

use super::*;

/// Every Google Chat conversation.
pub(super) async fn conversations(pool: &MySqlPool) -> Result<Vec<Conversation>> {
    let mut out = Vec::new();
    let gchat = sqlx::query(
        r"SELECT g.group_id AS id, g.name AS name, g.is_dm AS is_dm,
                 COALESCE(s.cnt, 0) AS cnt, s.last_ts_us AS last_ts_us,
                 CASE WHEN l.id IS NOT NULL THEN COALESCE(l.sender_name, '') END AS last_sender,
                 l.is_self AS last_out, l.text AS last_text
          FROM gchat_conversations g
          LEFT JOIN (
              SELECT group_id, COUNT(*) AS cnt, MAX(ts_us) AS last_ts_us
              FROM gchat_messages GROUP BY group_id
          ) s ON s.group_id = g.group_id
          LEFT JOIN gchat_messages l ON l.group_id = g.group_id AND l.ts_us = s.last_ts_us
          ORDER BY l.id DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut seen = std::collections::HashSet::new();
    for r in gchat {
        if !seen.insert(r.try_get::<String, _>("id")?) {
            continue;
        }
        let is_dm: i8 = r.try_get("is_dm")?;
        let last_us: Option<i64> = r.try_get("last_ts_us")?;
        out.push(Conversation {
            origin: Origin::Gchat,
            id: r.try_get("id")?,
            name: r.try_get("name")?,
            kind: kind_from_is_dm(is_dm != 0),
            network: None,
            message_count: r.try_get("cnt")?,
            last_ts: last_us.map(us_to_ms),
            last: LastMessage::from_row(&r, false)?,
        });
    }
    Ok(out)
}

pub(super) async fn page(
    pool: &MySqlPool,
    group_id: &str,
    cursor: Option<(i64, i64)>,
    limit: i64,
    dir: PageDir,
) -> Result<Fetched> {
    // The cursor is in native microseconds, so rows sharing a millisecond page
    // correctly.
    let (cur_ts, cur_id) = (cursor.map(|(ts, _)| ts), cursor.map(|(_, id)| id));
    let sql = match dir {
        PageDir::Older => {
            r"SELECT m.id AS id, m.ts_us AS ts_us, m.sender_name AS sender,
                 m.is_self AS is_self, m.text AS body,
                 m.reply_to_msg_id AS reply_to_msg_id, m.group_id AS group_id
          FROM gchat_messages m
          WHERE m.group_id = ?
            AND (? IS NULL OR m.ts_us < ? OR (m.ts_us = ? AND m.id < ?))
          ORDER BY m.ts_us DESC, m.id DESC
          LIMIT ?"
        }
        PageDir::Newer => {
            r"SELECT m.id AS id, m.ts_us AS ts_us, m.sender_name AS sender,
                 m.is_self AS is_self, m.text AS body,
                 m.reply_to_msg_id AS reply_to_msg_id, m.group_id AS group_id
          FROM gchat_messages m
          WHERE m.group_id = ?
            AND (? IS NULL OR m.ts_us > ? OR (m.ts_us = ? AND m.id > ?))
          ORDER BY m.ts_us ASC, m.id ASC
          LIMIT ?"
        }
        PageDir::AtAndNewer => {
            r"SELECT m.id AS id, m.ts_us AS ts_us, m.sender_name AS sender,
                 m.is_self AS is_self, m.text AS body,
                 m.reply_to_msg_id AS reply_to_msg_id, m.group_id AS group_id
          FROM gchat_messages m
          WHERE m.group_id = ?
            AND (? IS NULL OR m.ts_us > ? OR (m.ts_us = ? AND m.id >= ?))
          ORDER BY m.ts_us ASC, m.id ASC
          LIMIT ?"
        }
    };
    let rows = sqlx::query(sql)
        .bind(group_id)
        .bind(cur_ts)
        .bind(cur_ts)
        .bind(cur_ts)
        .bind(cur_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;

    let mut msgs = Vec::with_capacity(rows.len());
    let mut quoted: Vec<(String, String)> = Vec::new();
    let mut ids = Vec::with_capacity(rows.len());
    let mut keys = Vec::with_capacity(rows.len());
    for r in rows {
        let id: i64 = r.try_get("id")?;
        let ts_us: i64 = r.try_get("ts_us")?;
        keys.push((ts_us, id));
        let is_self: i8 = r.try_get("is_self")?;
        if let Some(target) = r.try_get::<Option<String>, _>("reply_to_msg_id")? {
            quoted.push((id.to_string(), target));
        }
        ids.push(id);
        msgs.push(Message::bare(
            id,
            us_to_ms(ts_us),
            r.try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            is_self != 0,
            r.try_get("body")?,
        ));
    }

    attach_replies(pool, group_id, &mut msgs, &quoted).await?;
    attach_attachments(pool, &mut msgs, &ids).await?;

    if !ids.is_empty() {
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT r.message_id, r.emoji, r.cnt,
                    GROUP_CONCAT(COALESCE(g.sender_name, a.reactor_id)
                                 ORDER BY COALESCE(g.sender_name, a.reactor_id)
                                 SEPARATOR 0x1f) AS who
               FROM gchat_reactions r
               LEFT JOIN gchat_reaction_authors a
                      ON a.message_id = r.message_id AND a.emoji = r.emoji
               LEFT JOIN (SELECT sender_id, MIN(sender_name) AS sender_name
                            FROM gchat_messages
                           WHERE sender_id IS NOT NULL GROUP BY sender_id) g
                      ON g.sender_id = a.reactor_id
              WHERE r.emoji IS NOT NULL AND r.message_id IN ({placeholders})
              GROUP BY r.message_id, r.emoji, r.cnt",
        );
        let mut q = sqlx::query(AssertSqlSafe(sql));
        for id in &ids {
            q = q.bind(id);
        }
        let rrows = q.fetch_all(pool).await?;
        for rr in rrows {
            let mid: i64 = rr.try_get("message_id")?;
            let emoji: String = rr.try_get("emoji")?;
            let count: i64 = rr.try_get("cnt")?;
            // Joined with 0x1f, since display names can contain commas.
            // `GROUP_CONCAT` truncates at `group_concat_max_len`, far above the
            // reactor counts here.
            let who: Vec<String> = rr
                .try_get::<Option<String>, _>("who")?
                .map(|s| s.split('\u{1f}').map(str::to_owned).collect())
                .unwrap_or_default();
            let mid = mid.to_string();
            if let Some(m) = msgs.iter_mut().find(|m| m.id == mid) {
                // `who` comes from a second capture (gchat-archive's sync.py,
                // loaded by import_gchat.py); empty means not yet resolved.
                m.reactions.push(Reaction { emoji, count, who });
            }
        }
    }

    Ok(Fetched::new(msgs, keys, dir))
}

/// Resolve Google Chat's quote-replies for one page, by the target's message id
/// within the group. Not `thread_id`, which is the topic: a DM message is its
/// own topic but can still quote another.
async fn attach_replies(
    pool: &MySqlPool,
    group_id: &str,
    msgs: &mut [Message],
    quoted: &[(String, String)],
) -> Result<()> {
    if quoted.is_empty() {
        return Ok(());
    }
    let targets = distinct(quoted.iter().map(|(_, q)| q.as_str()));
    let placeholders = vec!["?"; targets.len()].join(",");
    let sql = format!(
        "SELECT m.id AS id, m.msg_id AS msg_id, m.ts_us AS ts_us,
                m.sender_name AS sender, m.text AS body
           FROM gchat_messages m
          WHERE m.group_id = ? AND m.msg_id IN ({placeholders})",
    );
    let mut q = sqlx::query(AssertSqlSafe(sql)).bind(group_id);
    for t in &targets {
        q = q.bind(t);
    }
    let mut found: HashMap<String, ReplyTo> = HashMap::new();
    for r in q.fetch_all(pool).await? {
        let id: i64 = r.try_get("id")?;
        let ts_us: i64 = r.try_get("ts_us")?;
        let body: Option<String> = r.try_get("body")?;
        found.insert(
            r.try_get("msg_id")?,
            ReplyTo {
                id: Some(id.to_string()),
                // The cursor is in Google Chat's microseconds; `ts` is milliseconds.
                cursor: Some(encode_cursor(ts_us, id)),
                ts: Some(us_to_ms(ts_us)),
                sender: r.try_get::<Option<String>, _>("sender").ok().flatten(),
                excerpt: excerpt(body.as_deref()),
                // No deletion state is captured.
                deleted: false,
            },
        );
    }
    for (msg_id, target) in quoted {
        let Some(m) = msgs.iter_mut().find(|m| &m.id == msg_id) else {
            continue;
        };
        // An unresolved target still records that a reply happened.
        m.reply_to = Some(found.get(target).cloned().unwrap_or_else(ReplyTo::not_held));
    }
    Ok(())
}

/// The pictures and videos a Google Chat message carried.
///
/// Held bytes are whatever a harvest read; the download URL is session-bound.
/// A row without `stored_path` still draws, as a picture known and not held.
///
/// Attachment ids are per origin, so each origin has its own serving route and
/// the frontend picks by origin.
async fn attach_attachments(pool: &MySqlPool, msgs: &mut [Message], ids: &[i64]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT id, message_id, name, mime, stored_path
           FROM gchat_attachments WHERE message_id IN ({placeholders})
          ORDER BY id",
    );
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for id in ids {
        q = q.bind(id);
    }
    for r in q.fetch_all(pool).await? {
        let mid: i64 = r.try_get("message_id")?;
        let mid = mid.to_string();
        let mime: Option<String> = r.try_get("mime")?;
        let stored_path: Option<String> = r.try_get("stored_path")?;
        let att = Attachment {
            id: r.try_get::<i64, _>("id")?.to_string(),
            is_image: is_image(mime.as_deref()),
            content_type: mime,
            file_name: r.try_get("name")?,
            // Google Chat records pixel dimensions, not a byte count.
            size: None,
            available: stored_path.is_some(),
            // Fetching needs a URL the client mints while rendering.
            fetch: None,
        };
        if let Some(m) = msgs.iter_mut().find(|m| m.id == mid) {
            m.attachments.push(att);
        }
    }
    Ok(())
}

/// The bytes held for one Google Chat attachment.
pub async fn attachment_blob(
    pool: &MySqlPool,
    id: i64,
) -> Result<Option<(Option<String>, String)>> {
    let row = sqlx::query(
        "SELECT mime AS content_type, stored_path FROM gchat_attachments
          WHERE id = ? AND stored_path IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some(r) => Ok(Some((
            r.try_get("content_type")?,
            r.try_get("stored_path")?,
        ))),
        None => Ok(None),
    }
}

/// Google Chat messages whose text matches `like`, in one group when `id` is given.
pub(super) async fn search(
    pool: &MySqlPool,
    like: &str,
    id: Option<&str>,
    limit: i64,
) -> Result<Vec<SearchHit>> {
    let mut hits = Vec::new();
    let sql = if id.is_some() {
        r"SELECT m.id AS id, m.group_id AS cid, g.name AS cname, m.ts_us AS ts_us,
                 m.sender_name AS sender, m.text AS body
          FROM gchat_messages m
          LEFT JOIN gchat_conversations g ON g.group_id = m.group_id
          WHERE m.text LIKE ? AND m.group_id = ?
          ORDER BY m.ts_us DESC LIMIT ?"
    } else {
        r"SELECT m.id AS id, m.group_id AS cid, g.name AS cname, m.ts_us AS ts_us,
                 m.sender_name AS sender, m.text AS body
          FROM gchat_messages m
          LEFT JOIN gchat_conversations g ON g.group_id = m.group_id
          WHERE m.text LIKE ?
          ORDER BY m.ts_us DESC LIMIT ?"
    };
    let mut q = sqlx::query(sql).bind(like);
    if let Some(id) = id {
        q = q.bind(id);
    }
    let rows = q.bind(limit).fetch_all(pool).await?;
    for r in rows {
        let ts_us: i64 = r.try_get("ts_us")?;
        let id: i64 = r.try_get("id")?;
        hits.push(SearchHit {
            origin: Origin::Gchat,
            conversation_id: r.try_get("cid")?,
            conversation_name: r.try_get("cname")?,
            ts: us_to_ms(ts_us),
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            snippet: r.try_get::<Option<String>, _>("body")?.unwrap_or_default(),
            deleted: false, // not captured
            // Microseconds, unlike `ts`.
            cursor: encode_cursor(ts_us, id),
        });
    }
    Ok(hits)
}
