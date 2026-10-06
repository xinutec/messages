//! Signal: `messages`, with its quotes, edits, styles, previews, reactions,
//! attachments and receipts.

use std::collections::HashMap;

use anyhow::{Result, bail};
use sqlx::{AssertSqlSafe, MySqlPool, Row};

use super::*;

/// Every Signal conversation.
pub(super) async fn conversations(pool: &MySqlPool) -> Result<Vec<Conversation>> {
    let mut out = Vec::new();
    // An edit's revision rows are versions, not messages: the page leaves them
    // out, so the count does too.
    let signal = sqlx::query(
        r"SELECT c.thread_id AS id, c.type AS kind, c.name AS name,
                 COUNT(CASE WHEN m.edit_of_ts IS NULL THEN m.id END) AS cnt,
                 MAX(m.server_ts) AS last_ts
          FROM conversations c
          LEFT JOIN messages m ON m.thread_id = c.thread_id
          GROUP BY c.thread_id, c.type, c.name",
    )
    .fetch_all(pool)
    .await?;
    for r in signal {
        let kind: String = r.try_get("kind")?;
        let Some(kind) = ConversationKind::parse(&kind) else {
            bail!("conversations.type holds an unknown kind: {kind:?}");
        };
        out.push(Conversation {
            origin: Origin::Signal,
            id: r.try_get("id")?,
            name: r.try_get("name")?,
            kind,
            network: None,
            message_count: r.try_get("cnt")?,
            last_ts: r.try_get("last_ts")?,
        });
    }
    Ok(out)
}

pub(super) async fn page(
    pool: &MySqlPool,
    thread_id: &str,
    cursor: Option<(i64, i64)>,
    limit: i64,
    dir: PageDir,
) -> Result<Fetched> {
    let (cur_ts, cur_id) = (cursor.map(|(ts, _)| ts), cursor.map(|(_, id)| id));
    // Tie-broken by id so a page boundary never splits rows sharing a timestamp.
    // The first `?` doubles as the no-cursor guard.
    let sql = match dir {
        PageDir::Older => {
            r"SELECT m.id AS id, m.server_ts AS ts,
                 COALESCE(ct.display_name, m.sender_uuid) AS sender,
                 m.is_outgoing AS is_outgoing, m.body AS body,
                 m.deleted AS deleted, m.edited AS edited,
                 m.quote_target_ts AS quote_target_ts, m.quote_text AS quote_text,
                 COALESCE(qa.display_name, m.quote_author_uuid) AS quote_author
          FROM messages m
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          LEFT JOIN contacts qa ON qa.uuid = m.quote_author_uuid
          WHERE m.thread_id = ?
            AND m.edit_of_ts IS NULL
            AND (? IS NULL OR m.server_ts < ? OR (m.server_ts = ? AND m.id < ?))
          ORDER BY m.server_ts DESC, m.id DESC
          LIMIT ?"
        }
        PageDir::Newer => {
            r"SELECT m.id AS id, m.server_ts AS ts,
                 COALESCE(ct.display_name, m.sender_uuid) AS sender,
                 m.is_outgoing AS is_outgoing, m.body AS body,
                 m.deleted AS deleted, m.edited AS edited,
                 m.quote_target_ts AS quote_target_ts, m.quote_text AS quote_text,
                 COALESCE(qa.display_name, m.quote_author_uuid) AS quote_author
          FROM messages m
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          LEFT JOIN contacts qa ON qa.uuid = m.quote_author_uuid
          WHERE m.thread_id = ?
            AND m.edit_of_ts IS NULL
            AND (? IS NULL OR m.server_ts > ? OR (m.server_ts = ? AND m.id > ?))
          ORDER BY m.server_ts ASC, m.id ASC
          LIMIT ?"
        }
        PageDir::AtAndNewer => {
            r"SELECT m.id AS id, m.server_ts AS ts,
                 COALESCE(ct.display_name, m.sender_uuid) AS sender,
                 m.is_outgoing AS is_outgoing, m.body AS body,
                 m.deleted AS deleted, m.edited AS edited,
                 m.quote_target_ts AS quote_target_ts, m.quote_text AS quote_text,
                 COALESCE(qa.display_name, m.quote_author_uuid) AS quote_author
          FROM messages m
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          LEFT JOIN contacts qa ON qa.uuid = m.quote_author_uuid
          WHERE m.thread_id = ?
            AND m.edit_of_ts IS NULL
            AND (? IS NULL OR m.server_ts > ? OR (m.server_ts = ? AND m.id >= ?))
          ORDER BY m.server_ts ASC, m.id ASC
          LIMIT ?"
        }
    };
    let rows = sqlx::query(sql)
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
    let mut keys = Vec::with_capacity(rows.len());
    let mut quotes: Vec<Quote> = Vec::new();
    for r in rows {
        let id: i64 = r.try_get("id")?;
        let ts: i64 = r.try_get("ts")?;
        if let Some(target) = r.try_get::<Option<i64>, _>("quote_target_ts")? {
            quotes.push(Quote {
                msg_id: id.to_string(),
                target_ts: target,
                author: r.try_get("quote_author")?,
                text: r.try_get("quote_text")?,
            });
        }
        keys.push((ts, id));
        let is_outgoing: i8 = r.try_get("is_outgoing")?;
        let deleted: i8 = r.try_get("deleted")?;
        let edited: i8 = r.try_get("edited")?;
        ts_list.push(ts);
        ids.push(id);
        msgs.push(Message {
            deleted: deleted != 0,
            edited: edited != 0,
            ..Message::bare(
                id,
                ts,
                r.try_get("sender")?,
                is_outgoing != 0,
                r.try_get("body")?,
            )
        });
    }

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
                fetch: None,
            };
            if let Some(m) = msgs.iter_mut().find(|m| m.id == mid) {
                m.attachments.push(att);
            }
        }
    }

    // Reactions key on (thread_id, target_ts = the message's server_ts): distinct
    // current authors per emoji.
    if !ts_list.is_empty() {
        let placeholders = vec!["?"; ts_list.len()].join(",");
        // Names grouped in Rust rather than by `GROUP_CONCAT`, which truncates at
        // `group_concat_max_len` without an error.
        let sql = format!(
            "SELECT r.target_ts, r.emoji,
                    COALESCE(ct.display_name, r.author_uuid) AS who
             FROM reactions r
             LEFT JOIN contacts ct ON ct.uuid = r.author_uuid
             WHERE r.thread_id = ? AND r.removed = 0 AND r.emoji IS NOT NULL
               AND r.target_ts IN ({placeholders})
             ORDER BY r.target_ts, r.emoji, who",
        );
        let mut q = sqlx::query(AssertSqlSafe(sql)).bind(thread_id);
        for ts in &ts_list {
            q = q.bind(ts);
        }
        let rrows = q.fetch_all(pool).await?;
        // One author reacting twice with an emoji counts and is named once.
        let mut grouped: HashMap<(i64, String), Vec<String>> = HashMap::new();
        let mut order: Vec<(i64, String)> = Vec::new();
        for rr in rrows {
            let key = (rr.try_get("target_ts")?, rr.try_get("emoji")?);
            let who: String = rr.try_get("who")?;
            let names = grouped.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                Vec::new()
            });
            if !names.contains(&who) {
                names.push(who);
            }
        }
        for (target_ts, emoji) in order {
            let who = grouped
                .remove(&(target_ts, emoji.clone()))
                .unwrap_or_default();
            if let Some(m) = msgs.iter_mut().find(|m| m.ts == target_ts) {
                m.reactions.push(Reaction {
                    emoji,
                    count: who.len() as i64,
                    who,
                });
            }
        }
    }

    attach_replies(pool, thread_id, &mut msgs, &quotes).await?;
    attach_read(pool, &mut msgs).await?;

    Ok(Fetched::new(msgs, keys, dir))
}

/// A Signal quote: the target's timestamp, and its author and text as the quote
/// carries them.
struct Quote {
    msg_id: String,
    target_ts: i64,
    author: Option<String>,
    text: Option<String>,
}

/// Resolve Signal's quotes for one page. Signal quotes by timestamp; a target
/// the archive does not hold keeps what the quote itself says.
async fn attach_replies(
    pool: &MySqlPool,
    thread_id: &str,
    msgs: &mut [Message],
    quotes: &[Quote],
) -> Result<()> {
    if quotes.is_empty() {
        return Ok(());
    }
    let targets = distinct(quotes.iter().map(|q| q.target_ts));
    let placeholders = vec!["?"; targets.len()].join(",");
    let sql = format!(
        "SELECT m.id AS id, m.server_ts AS ts, m.body AS body, m.deleted AS deleted,
                COALESCE(ct.display_name, m.sender_uuid) AS sender
         FROM messages m
         LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
         WHERE m.thread_id = ? AND m.edit_of_ts IS NULL
           AND m.server_ts IN ({placeholders})",
    );
    let mut q = sqlx::query(AssertSqlSafe(sql)).bind(thread_id);
    for ts in &targets {
        q = q.bind(ts);
    }
    let mut found: HashMap<i64, ReplyTo> = HashMap::new();
    for r in q.fetch_all(pool).await? {
        let id: i64 = r.try_get("id")?;
        let ts: i64 = r.try_get("ts")?;
        let deleted: i8 = r.try_get("deleted")?;
        let deleted = deleted != 0;
        let body: Option<String> = r.try_get("body")?;
        found.entry(ts).or_insert_with(|| ReplyTo {
            id: Some(id.to_string()),
            cursor: Some(encode_cursor(ts, id)),
            ts: Some(ts),
            sender: r.try_get("sender").ok(),
            excerpt: if deleted {
                None
            } else {
                excerpt(body.as_deref())
            },
            deleted,
        });
    }
    for q in quotes {
        let Some(m) = msgs.iter_mut().find(|m| m.id == q.msg_id) else {
            continue;
        };
        m.reply_to = Some(found.get(&q.target_ts).cloned().unwrap_or_else(|| ReplyTo {
            // Not held: who and what, as the quote itself carries them.
            ts: Some(q.target_ts),
            sender: q.author.clone(),
            excerpt: excerpt(q.text.as_deref()),
            ..ReplyTo::not_held()
        }));
    }
    Ok(())
}

/// One stored version of a message: when it was sent, and what it said.
type Version = (i64, Option<String>);

/// Hang an edited Signal message's history on it, with the newest text as its
/// body and the original's position. The revision rows are excluded from the
/// page itself.
///
/// Returns, for each edited message, the row id of the revision whose text it
/// now shows.
pub(super) async fn attach_edits(
    pool: &MySqlPool,
    thread_id: &str,
    msgs: &mut [Message],
) -> Result<HashMap<String, i64>> {
    let mut shown = HashMap::new();
    let originals: Vec<i64> = msgs.iter().filter(|m| m.edited).map(|m| m.ts).collect();
    if originals.is_empty() {
        return Ok(shown);
    }
    let placeholders = vec!["?"; originals.len()].join(",");
    // Scoped to the thread: `edit_of_ts` is a timestamp, and two threads can
    // share one.
    let sql = format!(
        "SELECT id, edit_of_ts, server_ts, body FROM messages
         WHERE thread_id = ? AND edit_of_ts IN ({placeholders})
         ORDER BY server_ts ASC",
    );
    let mut q = sqlx::query(AssertSqlSafe(sql)).bind(thread_id);
    for ts in &originals {
        q = q.bind(ts);
    }

    // Group first, then assign: each version's time comes from the row before
    // it, not the row carrying its text.
    let mut revisions: Vec<(i64, Vec<Version>)> = Vec::new();
    let mut newest_row: HashMap<i64, i64> = HashMap::new();
    for row in q.fetch_all(pool).await? {
        let of: i64 = row.try_get("edit_of_ts")?;
        newest_row.insert(of, row.try_get("id")?);
        let ts: i64 = row.try_get("server_ts")?;
        let body: Option<String> = row.try_get("body")?;
        match revisions.iter_mut().find(|(k, _)| *k == of) {
            Some((_, list)) => list.push((ts, body)),
            None => revisions.push((of, vec![(ts, body)])),
        }
    }

    for (of, list) in revisions {
        let Some(m) = msgs.iter_mut().find(|m| m.ts == of) else {
            continue;
        };
        let Some((_, newest)) = list.last().cloned() else {
            continue;
        };
        // Earlier versions, in order: the original's text, then every revision
        // but the newest, each stamped with when it was sent.
        let Some((_, earlier)) = list.split_last() else {
            continue;
        };
        let older_bodies =
            std::iter::once(m.body.clone()).chain(earlier.iter().map(|(_, b)| b.clone()));
        let stamps = std::iter::once(m.ts).chain(earlier.iter().map(|(t, _)| *t));
        m.edits = stamps
            .zip(older_bodies)
            .map(|(ts, body)| MessageEdit { ts, body })
            .collect();
        m.body = newest;
        if let Some(row) = newest_row.get(&of) {
            shown.insert(m.id.clone(), *row);
        }
    }
    Ok(shown)
}

/// A Signal text style, by Signal's own name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalStyle {
    Bold,
    Italic,
    Strikethrough,
    Monospace,
    Spoiler,
}

impl SignalStyle {
    /// Parse a `signal_text_styles.style` value; `None` for one this build does
    /// not know.
    fn parse(s: &str) -> Option<Self> {
        match s {
            "BOLD" => Some(SignalStyle::Bold),
            "ITALIC" => Some(SignalStyle::Italic),
            "STRIKETHROUGH" => Some(SignalStyle::Strikethrough),
            "MONOSPACE" => Some(SignalStyle::Monospace),
            "SPOILER" => Some(SignalStyle::Spoiler),
            _ => None,
        }
    }

    /// The viewer's entity kind for it.
    fn kind(self) -> &'static str {
        match self {
            SignalStyle::Bold => "bold",
            SignalStyle::Italic => "italic",
            SignalStyle::Strikethrough => "strike",
            SignalStyle::Monospace => "code",
            SignalStyle::Spoiler => "spoiler",
        }
    }
}

/// Styled runs for this page's Signal messages, from the row whose text each
/// shows: `shown` maps an edited message to its newest revision.
pub(super) async fn attach_styles(
    pool: &MySqlPool,
    msgs: &mut [Message],
    shown: &HashMap<String, i64>,
) -> Result<()> {
    let rows = shown_rows(msgs, shown);
    if rows.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; rows.len()].join(",");
    let sql = format!(
        "SELECT message_id, style, start_utf16, length_utf16 FROM signal_text_styles
          WHERE message_id IN ({placeholders})
          ORDER BY message_id, start_utf16, length_utf16, style",
    );
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for (_, row) in &rows {
        q = q.bind(row);
    }
    for r in q.fetch_all(pool).await? {
        let row: i64 = r.try_get("message_id")?;
        let style: String = r.try_get("style")?;
        let start: i32 = r.try_get("start_utf16")?;
        let length: i32 = r.try_get("length_utf16")?;
        for (i, _) in rows.iter().filter(|(_, rr)| *rr == row) {
            msgs[*i].entities.push(Entity {
                // An unknown style passes through, and renders as plain text.
                kind: SignalStyle::parse(&style).map_or(style.clone(), |st| st.kind().to_string()),
                offset: start.into(),
                length: length.into(),
                url: None,
            });
        }
    }
    Ok(())
}

/// Each message's index on the page, with the row whose text it shows.
fn shown_rows(msgs: &[Message], shown: &HashMap<String, i64>) -> Vec<(usize, i64)> {
    msgs.iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let row = shown.get(&m.id).copied().or_else(|| m.id.parse().ok())?;
            Some((i, row))
        })
        .collect()
}

/// Name the people this page's Signal messages mention. Signal sends a mention
/// as U+FFFC in the text; each becomes `@` and the name a sender would get, as
/// a `mention` run, and the runs after it move with the text. Run after
/// `attach_styles`, whose offsets it moves.
pub(super) async fn attach_mentions(
    pool: &MySqlPool,
    msgs: &mut [Message],
    shown: &HashMap<String, i64>,
) -> Result<()> {
    let rows = shown_rows(msgs, shown);
    if rows.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; rows.len()].join(",");
    // Last first, so naming one leaves the offsets of those before it alone.
    let sql = format!(
        "SELECT x.message_id, x.start_utf16, x.length_utf16,
                COALESCE(ct.display_name, x.uuid) AS who
           FROM signal_mentions x
           LEFT JOIN contacts ct ON ct.uuid = x.uuid
          WHERE x.message_id IN ({placeholders})
          ORDER BY x.message_id, x.start_utf16 DESC",
    );
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for (_, row) in &rows {
        q = q.bind(row);
    }
    for r in q.fetch_all(pool).await? {
        let row: i64 = r.try_get("message_id")?;
        let start: i32 = r.try_get("start_utf16")?;
        let length: i32 = r.try_get("length_utf16")?;
        let who: String = r.try_get("who")?;
        for (i, _) in rows.iter().filter(|(_, rr)| *rr == row) {
            name_mention(&mut msgs[*i], start.into(), length.into(), &who);
        }
    }
    Ok(())
}

/// Replace the placeholder at `start..start + length` (UTF-16) with `@who`.
/// Anything else there is left alone: the text has moved, and a name in the
/// wrong place is worse than a placeholder.
fn name_mention(m: &mut Message, start: i64, length: i64, who: &str) {
    let Some(body) = m.body.as_mut() else { return };
    let (Some(a), Some(b)) = (byte_at(body, start), byte_at(body, start + length)) else {
        return;
    };
    if a == b || body[a..b].chars().any(|c| c != '\u{FFFC}') {
        return;
    }
    let name = format!("@{who}");
    let named = name.encode_utf16().count() as i64;
    body.replace_range(a..b, &name);
    let (end, moved) = (start + length, named - length);
    for e in &mut m.entities {
        if e.offset >= end {
            e.offset += moved;
        } else if e.offset + e.length >= end && e.offset <= start {
            e.length += moved;
        }
    }
    m.entities.push(Entity {
        kind: "mention".to_string(),
        offset: start,
        length: named,
        url: None,
    });
}

/// The byte index of UTF-16 offset `at` in `s`, if one falls there.
fn byte_at(s: &str, at: i64) -> Option<usize> {
    let mut units = 0i64;
    for (i, c) in s.char_indices() {
        if units == at {
            return Some(i);
        }
        units += c.len_utf16() as i64;
    }
    (units == at).then_some(s.len())
}

/// Link previews for this page's Signal messages, in the order sent.
pub(super) async fn attach_previews(pool: &MySqlPool, msgs: &mut [Message]) -> Result<()> {
    let ids: Vec<i64> = msgs.iter().filter_map(|m| m.id.parse().ok()).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT id, message_id, url, title, description, image_path
           FROM signal_link_previews
          WHERE message_id IN ({placeholders})
          ORDER BY message_id, position",
    );
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for id in &ids {
        q = q.bind(id);
    }
    for r in q.fetch_all(pool).await? {
        let message_id: i64 = r.try_get("message_id")?;
        let message_id = message_id.to_string();
        if let Some(m) = msgs.iter_mut().find(|m| m.id == message_id) {
            let id: i64 = r.try_get("id")?;
            let held: Option<String> = r.try_get("image_path")?;
            m.previews.push(LinkPreview {
                url: r.try_get("url")?,
                title: r.try_get("title")?,
                description: r.try_get("description")?,
                image: held.map(|_| id.to_string()),
            });
        }
    }
    Ok(())
}

/// Signal delivery and read state for this page's outgoing messages.
///
/// Receipts from the message's own sender are dropped: reading a thread on a
/// linked device syncs a read of my own messages too.
///
/// A message with no receipt is `Sent` only if it postdates the first receipt
/// ever captured; before that, the archive cannot say.
async fn attach_read(pool: &MySqlPool, msgs: &mut [Message]) -> Result<()> {
    let ids: Vec<i64> = msgs
        .iter()
        .filter(|m| m.is_outgoing)
        .filter_map(|m| m.id.parse().ok())
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let floor_ms: Option<i64> =
        sqlx::query_scalar("SELECT UNIX_TIMESTAMP(MIN(observed_at)) * 1000 FROM signal_receipts")
            .fetch_optional(pool)
            .await?
            .flatten();

    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT m.id AS id, r.kind AS kind, r.when_ts AS when_ts,
                COALESCE(ct.display_name, r.author_uuid) AS who
         FROM signal_receipts r
         JOIN messages m ON m.server_ts = r.target_ts
         LEFT JOIN contacts ct ON ct.uuid = r.author_uuid
         WHERE m.id IN ({placeholders}) AND r.author_uuid <> m.sender_uuid
         ORDER BY r.when_ts",
    );
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for id in &ids {
        q = q.bind(id);
    }
    let mut best: HashMap<String, DeliveryState> = HashMap::new();
    let mut read_by: HashMap<String, Vec<ReadBy>> = HashMap::new();
    for row in q.fetch_all(pool).await? {
        let id: i64 = row.try_get("id")?;
        let id = id.to_string();
        let kind: String = row.try_get("kind")?;
        let state = match kind.as_str() {
            "delivery" => DeliveryState::Delivered,
            "read" => DeliveryState::Read,
            "viewed" => DeliveryState::Viewed,
            // An unknown kind is a schema change this build has not seen.
            other => bail!("signal_receipts.kind holds an unknown kind: {other:?}"),
        };
        let slot = best.entry(id.clone()).or_insert(DeliveryState::Sent);
        *slot = (*slot).max(state);
        if state >= DeliveryState::Read {
            // Ordered by `when_ts`; a person who read and then viewed is named once.
            let who: String = row.try_get("who")?;
            let entry = read_by.entry(id).or_default();
            if !entry.iter().any(|r| r.who == who) {
                entry.push(ReadBy {
                    who,
                    at: row.try_get("when_ts")?,
                });
            }
        }
    }
    for m in msgs.iter_mut().filter(|m| m.is_outgoing) {
        match best.get(&m.id) {
            Some(state) => {
                m.delivery = Some(Delivery {
                    state: *state,
                    read_by: read_by.remove(&m.id).unwrap_or_default(),
                });
            }
            // Nothing came back: meaningful only if we were listening then.
            None if floor_ms.is_some_and(|f| m.ts >= f) => {
                m.delivery = Some(Delivery {
                    state: DeliveryState::Sent,
                    read_by: Vec::new(),
                });
            }
            None => {}
        }
    }
    Ok(())
}

/// Stored location and content type of a Signal attachment's bytes, if held.
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

/// A link preview's stored picture: its content type and where it was stored.
pub async fn preview_image_blob(
    pool: &MySqlPool,
    id: i64,
) -> Result<Option<(Option<String>, String)>> {
    let row = sqlx::query(
        "SELECT image_content_type, image_path FROM signal_link_previews
          WHERE id = ? AND image_path IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some(r) => Ok(Some((
            r.try_get("image_content_type")?,
            r.try_get("image_path")?,
        ))),
        None => Ok(None),
    }
}

/// Signal messages whose text matches `like`, in one thread when `id` is given.
pub(super) async fn search(
    pool: &MySqlPool,
    like: &str,
    id: Option<&str>,
    limit: i64,
) -> Result<Vec<SearchHit>> {
    let mut hits = Vec::new();
    // One literal per scope, not a built string.
    //
    // A match in an edit's revision row is reported as the message it revises,
    // where the thread shows it, and a message matching in several versions is
    // one hit: the newest.
    let sql = if id.is_some() {
        r"SELECT COALESCE(o.id, m.id) AS id, m.thread_id AS cid, c.name AS cname,
                 COALESCE(o.server_ts, m.server_ts) AS ts,
                 COALESCE(ct.display_name, m.sender_uuid) AS sender, m.body AS body,
                 COALESCE(o.deleted, m.deleted) AS deleted
          FROM messages m
          LEFT JOIN messages o
                 ON m.edit_of_ts IS NOT NULL AND o.thread_id = m.thread_id
                AND o.sender_uuid = m.sender_uuid AND o.server_ts = m.edit_of_ts
                AND o.edit_of_ts IS NULL
          LEFT JOIN conversations c ON c.thread_id = m.thread_id
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          WHERE m.body LIKE ? AND m.thread_id = ?
          ORDER BY m.server_ts DESC LIMIT ?"
    } else {
        r"SELECT COALESCE(o.id, m.id) AS id, m.thread_id AS cid, c.name AS cname,
                 COALESCE(o.server_ts, m.server_ts) AS ts,
                 COALESCE(ct.display_name, m.sender_uuid) AS sender, m.body AS body,
                 COALESCE(o.deleted, m.deleted) AS deleted
          FROM messages m
          LEFT JOIN messages o
                 ON m.edit_of_ts IS NOT NULL AND o.thread_id = m.thread_id
                AND o.sender_uuid = m.sender_uuid AND o.server_ts = m.edit_of_ts
                AND o.edit_of_ts IS NULL
          LEFT JOIN conversations c ON c.thread_id = m.thread_id
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          WHERE m.body LIKE ?
          ORDER BY m.server_ts DESC LIMIT ?"
    };
    let mut q = sqlx::query(sql).bind(like);
    if let Some(id) = id {
        q = q.bind(id);
    }
    let rows = q.bind(limit).fetch_all(pool).await?;
    let mut seen = std::collections::HashSet::new();
    for r in rows {
        let deleted: i8 = r.try_get("deleted")?;
        let id: i64 = r.try_get("id")?;
        if !seen.insert(id) {
            continue;
        }
        let ts: i64 = r.try_get("ts")?;
        hits.push(SearchHit {
            origin: Origin::Signal,
            conversation_id: r.try_get("cid")?,
            conversation_name: r.try_get("cname")?,
            ts,
            sender: r.try_get("sender")?,
            snippet: r.try_get::<Option<String>, _>("body")?.unwrap_or_default(),
            deleted: deleted != 0,
            // Signal's native unit is milliseconds, the same as `ts`.
            cursor: encode_cursor(ts, id),
        });
    }
    Ok(hits)
}
