//! Read-only queries over the message archive, normalising the two origins
//! (Signal + Google Chat) into one shape for the UI.
//!
//! The origins differ: Signal keeps per-author reaction *events* (add/remove)
//! and message edit/delete flags; Google Chat keeps *aggregated* emoji counts,
//! Google threading and a numeric sender id. This module hides those behind a
//! common `Conversation` / `Message` so the frontend has one model. All access
//! is SELECT-only; nothing here writes to the archive tables.

use anyhow::Result;
use serde::Serialize;
use sqlx::{AssertSqlSafe, MySqlPool, Row};

pub const ORIGIN_SIGNAL: &str = "signal";
pub const ORIGIN_GCHAT: &str = "gchat";

pub fn valid_origin(origin: &str) -> bool {
    origin == ORIGIN_SIGNAL || origin == ORIGIN_GCHAT
}

/// Google Chat stores microsecond timestamps; the unified API uses milliseconds.
pub fn us_to_ms(us: i64) -> i64 {
    us / 1000
}

/// Conversation kind from the gchat `is_dm` flag.
pub fn kind_from_is_dm(is_dm: bool) -> &'static str {
    if is_dm { "dm" } else { "group" }
}

/// Escape a user search term for a SQL `LIKE` (so `%` and `_` are literal). The
/// query still binds the result as a parameter; this only neutralises wildcards.
pub fn escape_like(q: &str) -> String {
    format!(
        "%{}%",
        q.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

/// Opaque pagination cursor: the `(native_ts, id)` of the last (oldest) row a
/// page returned, so the next page resumes strictly before it. Two things matter:
/// the id tie-breaker (messages sharing a timestamp would otherwise be skipped
/// when a page boundary splits them), and keeping each origin's *native* ts
/// precision (Signal ms, Google Chat µs) — a millisecond-only cursor drops gchat
/// rows that share a millisecond. The value is minted and parsed here; callers
/// (and the frontend) treat it as opaque.
pub fn encode_cursor(native_ts: i64, id: i64) -> String {
    format!("{native_ts}_{id}")
}

/// Parse a cursor minted by [`encode_cursor`]; None for anything malformed (the
/// caller then just starts from the newest page).
pub fn parse_cursor(s: &str) -> Option<(i64, i64)> {
    let (ts, id) = s.split_once('_')?;
    Some((ts.parse().ok()?, id.parse().ok()?))
}

#[derive(Serialize)]
pub struct Conversation {
    pub origin: String,
    pub id: String,
    pub name: Option<String>,
    pub kind: String, // "dm" | "group"
    pub message_count: i64,
    pub last_ts: Option<i64>, // ms epoch
}

#[derive(Serialize)]
pub struct Reaction {
    pub emoji: String,
    pub count: i64,
}

#[derive(Serialize)]
pub struct Attachment {
    pub id: String,
    pub content_type: Option<String>,
    pub file_name: Option<String>,
    pub size: Option<i64>,
    /// Whether the bytes are present (downloaded to the PVC). Metadata-only
    /// history rows are `false` — the UI shows them but can't fetch the blob.
    pub available: bool,
    pub is_image: bool,
}

#[derive(Serialize)]
pub struct Message {
    pub id: String,
    pub ts: i64, // ms epoch
    pub sender: String,
    pub is_outgoing: bool,
    pub body: Option<String>,
    pub deleted: bool,
    pub edited: bool,
    pub reactions: Vec<Reaction>,
    pub attachments: Vec<Attachment>,
}

fn is_image(ct: Option<&str>) -> bool {
    ct.is_some_and(|c| c.starts_with("image/"))
}

/// Stored location + content-type for an attachment blob, if its bytes exist.
/// Used by the serving endpoint; returns None when unknown or metadata-only.
pub async fn attachment_blob(
    pool: &MySqlPool,
    id: i64,
) -> Result<Option<(Option<String>, String)>> {
    let row = sqlx::query(
        "SELECT content_type, stored_path FROM attachments WHERE id = ? AND stored_path IS NOT NULL",
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

#[derive(Serialize)]
pub struct MessagesPage {
    pub messages: Vec<Message>, // ascending by ts
    pub has_more: bool,
    pub next_cursor: Option<String>, // opaque cursor to fetch the next older page
}

/// All conversations across both origins, newest activity first.
pub async fn list_conversations(pool: &MySqlPool) -> Result<Vec<Conversation>> {
    let mut out = Vec::new();

    let signal = sqlx::query(
        r"SELECT c.thread_id AS id, c.type AS kind, c.name AS name,
                 COUNT(m.id) AS cnt, MAX(m.server_ts) AS last_ts
          FROM conversations c
          LEFT JOIN messages m ON m.thread_id = c.thread_id
          GROUP BY c.thread_id, c.type, c.name",
    )
    .fetch_all(pool)
    .await?;
    for r in signal {
        out.push(Conversation {
            origin: ORIGIN_SIGNAL.into(),
            id: r.try_get("id")?,
            name: r.try_get("name")?,
            kind: r.try_get("kind")?,
            message_count: r.try_get("cnt")?,
            last_ts: r.try_get("last_ts")?,
        });
    }

    let gchat = sqlx::query(
        r"SELECT g.group_id AS id, g.name AS name, g.is_dm AS is_dm,
                 COUNT(m.id) AS cnt, MAX(m.ts_us) AS last_ts_us
          FROM gchat_conversations g
          LEFT JOIN gchat_messages m ON m.group_id = g.group_id
          GROUP BY g.group_id, g.name, g.is_dm",
    )
    .fetch_all(pool)
    .await?;
    for r in gchat {
        let is_dm: i8 = r.try_get("is_dm")?;
        let last_us: Option<i64> = r.try_get("last_ts_us")?;
        out.push(Conversation {
            origin: ORIGIN_GCHAT.into(),
            id: r.try_get("id")?,
            name: r.try_get("name")?,
            kind: kind_from_is_dm(is_dm != 0).into(),
            message_count: r.try_get("cnt")?,
            last_ts: last_us.map(us_to_ms),
        });
    }

    out.sort_by_key(|c| std::cmp::Reverse(c.last_ts)); // newest activity first
    Ok(out)
}

/// One page of a conversation, oldest→newest, with reactions attached. `cursor`
/// (from a previous page's `next_cursor`) pages backwards in time; None starts at
/// the most recent. The per-origin fetchers mint `next_cursor` from their own
/// native ts, so it round-trips at full precision.
pub async fn messages_page(
    pool: &MySqlPool,
    origin: &str,
    id: &str,
    cursor: Option<(i64, i64)>,
    limit: i64,
) -> Result<MessagesPage> {
    // Each fetcher returns its page (DESC, newest first) plus the cursor for the
    // next older page — it alone knows the native ts unit + row id.
    let (mut msgs, next_cursor) = match origin {
        ORIGIN_SIGNAL => signal_messages(pool, id, cursor, limit).await?,
        ORIGIN_GCHAT => gchat_messages(pool, id, cursor, limit).await?,
        _ => (Vec::new(), None),
    };
    let has_more = msgs.len() as i64 == limit;
    msgs.reverse(); // present ascending
    Ok(MessagesPage {
        messages: msgs,
        has_more,
        next_cursor,
    })
}

async fn signal_messages(
    pool: &MySqlPool,
    thread_id: &str,
    cursor: Option<(i64, i64)>,
    limit: i64,
) -> Result<(Vec<Message>, Option<String>)> {
    let (cur_ts, cur_id) = (cursor.map(|(ts, _)| ts), cursor.map(|(_, id)| id));
    // Newest first, tie-broken by id so a page boundary never splits a run of
    // messages sharing a server_ts. The first `?` (cur_ts) doubles as the
    // "no cursor → whole thread" guard.
    let rows = sqlx::query(
        r"SELECT m.id AS id, m.server_ts AS ts,
                 COALESCE(ct.profile_name, m.sender_uuid) AS sender,
                 m.is_outgoing AS is_outgoing, m.body AS body,
                 m.deleted AS deleted, m.edited AS edited
          FROM messages m
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          WHERE m.thread_id = ?
            AND (? IS NULL OR m.server_ts < ? OR (m.server_ts = ? AND m.id < ?))
          ORDER BY m.server_ts DESC, m.id DESC
          LIMIT ?",
    )
    .bind(thread_id)
    .bind(cur_ts)
    .bind(cur_ts)
    .bind(cur_ts)
    .bind(cur_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut msgs = Vec::with_capacity(rows.len());
    let mut ts_list = Vec::with_capacity(rows.len());
    let mut ids = Vec::with_capacity(rows.len());
    for r in rows {
        let id: i64 = r.try_get("id")?;
        let ts: i64 = r.try_get("ts")?;
        let is_outgoing: i8 = r.try_get("is_outgoing")?;
        let deleted: i8 = r.try_get("deleted")?;
        let edited: i8 = r.try_get("edited")?;
        ts_list.push(ts);
        ids.push(id);
        msgs.push(Message {
            id: id.to_string(),
            ts,
            sender: r.try_get("sender")?,
            is_outgoing: is_outgoing != 0,
            body: r.try_get("body")?,
            deleted: deleted != 0,
            edited: edited != 0,
            reactions: Vec::new(),
            attachments: Vec::new(),
        });
    }

    // Attachments (Signal only) — metadata for the page's messages; `available`
    // marks the ones whose bytes were downloaded to the PVC.
    if !ids.is_empty() {
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT id, message_id, content_type, file_name, size_bytes, stored_path
             FROM attachments WHERE message_id IN ({placeholders})",
        );
        let mut q = sqlx::query(AssertSqlSafe(sql));
        for id in &ids {
            q = q.bind(id);
        }
        for ar in q.fetch_all(pool).await? {
            let mid: i64 = ar.try_get("message_id")?;
            let mid = mid.to_string();
            let content_type: Option<String> = ar.try_get("content_type")?;
            let stored_path: Option<String> = ar.try_get("stored_path")?;
            let att = Attachment {
                id: ar.try_get::<i64, _>("id")?.to_string(),
                is_image: is_image(content_type.as_deref()),
                content_type,
                file_name: ar.try_get("file_name")?,
                size: ar.try_get("size_bytes")?,
                available: stored_path.is_some(),
            };
            if let Some(m) = msgs.iter_mut().find(|m| m.id == mid) {
                m.attachments.push(att);
            }
        }
    }

    // Reactions key on (thread_id, target_ts=message server_ts). Approximate the
    // live state as distinct non-removed authors per emoji (ignores the rare
    // add-then-remove of the same author within the page).
    if !ts_list.is_empty() {
        let placeholders = vec!["?"; ts_list.len()].join(",");
        let sql = format!(
            "SELECT target_ts, emoji, COUNT(DISTINCT author_uuid) AS cnt
             FROM reactions
             WHERE thread_id = ? AND removed = 0 AND emoji IS NOT NULL
               AND target_ts IN ({placeholders})
             GROUP BY target_ts, emoji",
        );
        // `sql` is a fixed template with a computed count of `?` placeholders and
        // no interpolated data; all values are bound. Safe to assert.
        let mut q = sqlx::query(AssertSqlSafe(sql)).bind(thread_id);
        for ts in &ts_list {
            q = q.bind(ts);
        }
        let rrows = q.fetch_all(pool).await?;
        for rr in rrows {
            let target_ts: i64 = rr.try_get("target_ts")?;
            let emoji: String = rr.try_get("emoji")?;
            let count: i64 = rr.try_get("cnt")?;
            if let Some(m) = msgs.iter_mut().find(|m| m.ts == target_ts) {
                m.reactions.push(Reaction { emoji, count });
            }
        }
    }

    // The oldest row (last, since DESC) is the cursor for the next older page.
    let next_cursor = ids
        .last()
        .zip(ts_list.last())
        .map(|(&id, &ts)| encode_cursor(ts, id));
    Ok((msgs, next_cursor))
}

async fn gchat_messages(
    pool: &MySqlPool,
    group_id: &str,
    cursor: Option<(i64, i64)>,
    limit: i64,
) -> Result<(Vec<Message>, Option<String>)> {
    // The cursor carries the native µs ts (not the ms the UI sees), so paging
    // never skips rows that share a millisecond; id tie-breaks an exact µs match.
    let (cur_ts, cur_id) = (cursor.map(|(ts, _)| ts), cursor.map(|(_, id)| id));
    let rows = sqlx::query(
        r"SELECT m.id AS id, m.ts_us AS ts_us, m.sender_name AS sender,
                 m.is_self AS is_self, m.text AS body
          FROM gchat_messages m
          WHERE m.group_id = ?
            AND (? IS NULL OR m.ts_us < ? OR (m.ts_us = ? AND m.id < ?))
          ORDER BY m.ts_us DESC, m.id DESC
          LIMIT ?",
    )
    .bind(group_id)
    .bind(cur_ts)
    .bind(cur_ts)
    .bind(cur_ts)
    .bind(cur_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut msgs = Vec::with_capacity(rows.len());
    let mut ids = Vec::with_capacity(rows.len());
    let mut oldest: Option<(i64, i64)> = None; // (ts_us, id) of the last row seen
    for r in rows {
        let id: i64 = r.try_get("id")?;
        let ts_us: i64 = r.try_get("ts_us")?;
        let is_self: i8 = r.try_get("is_self")?;
        ids.push(id);
        oldest = Some((ts_us, id));
        msgs.push(Message {
            id: id.to_string(),
            ts: us_to_ms(ts_us),
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            is_outgoing: is_self != 0,
            body: r.try_get("body")?,
            deleted: false,
            edited: false,
            reactions: Vec::new(),
            attachments: Vec::new(), // Google Chat export carries no attachments
        });
    }

    if !ids.is_empty() {
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT message_id, emoji, cnt FROM gchat_reactions
             WHERE emoji IS NOT NULL AND message_id IN ({placeholders})",
        );
        // Fixed template + computed placeholder count, values bound — safe.
        let mut q = sqlx::query(AssertSqlSafe(sql));
        for id in &ids {
            q = q.bind(id);
        }
        let rrows = q.fetch_all(pool).await?;
        for rr in rrows {
            let mid: i64 = rr.try_get("message_id")?;
            let emoji: String = rr.try_get("emoji")?;
            let count: i64 = rr.try_get("cnt")?;
            let mid = mid.to_string();
            if let Some(m) = msgs.iter_mut().find(|m| m.id == mid) {
                m.reactions.push(Reaction { emoji, count });
            }
        }
    }

    let next_cursor = oldest.map(|(ts_us, id)| encode_cursor(ts_us, id));
    Ok((msgs, next_cursor))
}

#[derive(Serialize)]
pub struct SearchHit {
    pub origin: String,
    pub conversation_id: String,
    pub conversation_name: Option<String>,
    pub ts: i64,
    pub sender: String,
    pub snippet: String,
}

/// Simple substring search across both origins' message text. Newest first.
pub async fn search(pool: &MySqlPool, q: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let like = escape_like(q);
    let mut hits = Vec::new();

    let srows = sqlx::query(
        r"SELECT m.thread_id AS cid, c.name AS cname, m.server_ts AS ts,
                 COALESCE(ct.profile_name, m.sender_uuid) AS sender, m.body AS body
          FROM messages m
          LEFT JOIN conversations c ON c.thread_id = m.thread_id
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          WHERE m.deleted = 0 AND m.body LIKE ?
          ORDER BY m.server_ts DESC LIMIT ?",
    )
    .bind(&like)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    for r in srows {
        hits.push(SearchHit {
            origin: ORIGIN_SIGNAL.into(),
            conversation_id: r.try_get("cid")?,
            conversation_name: r.try_get("cname")?,
            ts: r.try_get("ts")?,
            sender: r.try_get("sender")?,
            snippet: r.try_get::<Option<String>, _>("body")?.unwrap_or_default(),
        });
    }

    let grows = sqlx::query(
        r"SELECT m.group_id AS cid, g.name AS cname, m.ts_us AS ts_us,
                 m.sender_name AS sender, m.text AS body
          FROM gchat_messages m
          LEFT JOIN gchat_conversations g ON g.group_id = m.group_id
          WHERE m.text LIKE ?
          ORDER BY m.ts_us DESC LIMIT ?",
    )
    .bind(&like)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    for r in grows {
        let ts_us: i64 = r.try_get("ts_us")?;
        hits.push(SearchHit {
            origin: ORIGIN_GCHAT.into(),
            conversation_id: r.try_get("cid")?,
            conversation_name: r.try_get("cname")?,
            ts: us_to_ms(ts_us),
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            snippet: r.try_get::<Option<String>, _>("body")?.unwrap_or_default(),
        });
    }

    hits.sort_by_key(|h| std::cmp::Reverse(h.ts)); // newest first
    hits.truncate(limit as usize);
    Ok(hits)
}
