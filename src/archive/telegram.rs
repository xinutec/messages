//! Telegram: `telegram_*`, with media fetched on request, reactions, read marks,
//! formatting entities and edit history.

use std::collections::HashMap;

use anyhow::{Result, bail};
use sqlx::{AssertSqlSafe, MySqlPool, Row};

use super::*;

/// Every Telegram conversation.
pub(super) async fn conversations(pool: &MySqlPool) -> Result<Vec<Conversation>> {
    let mut out = Vec::new();
    // Telegram counts only `kind = 'message'`. Aggregated in a derived table and
    // then joined, so MariaDB answers from `idx_tg_conv_kind_ts`.
    let telegram = sqlx::query(
        r"SELECT t.id AS id, t.kind AS kind, t.name AS name,
                 COALESCE(s.cnt, 0) AS cnt, s.last_ts AS last_ts,
                 l.deleted AS last_deleted,
                 CASE WHEN l.id IS NOT NULL THEN COALESCE(l.sender_name, '') END AS last_sender,
                 l.is_outgoing AS last_out, l.text AS last_text, l.media_mime AS last_media,
                 CASE WHEN rd.read_id IS NULL THEN 0 ELSE (
                     SELECT COUNT(*) FROM telegram_messages u
                      WHERE u.conversation_id = t.id AND u.msg_id > rd.read_id
                        AND u.kind = 'message' AND u.is_outgoing = 0) END AS unread
          FROM telegram_conversations t
          LEFT JOIN (
              SELECT conversation_id, COUNT(*) AS cnt, MAX(sent_at) AS last_ts
              FROM telegram_messages
              WHERE kind = 'message'
              GROUP BY conversation_id
          ) s ON s.conversation_id = t.id
          LEFT JOIN telegram_messages l
                 ON l.conversation_id = t.id AND l.kind = 'message' AND l.sent_at = s.last_ts
          LEFT JOIN (
              SELECT conversation_id, MAX(max_id) AS read_id
              FROM telegram_read_marks WHERE direction = 'inbox'
              GROUP BY conversation_id
          ) rd ON rd.conversation_id = t.id
          ORDER BY l.id DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut seen = std::collections::HashSet::new();
    for r in telegram {
        if !seen.insert(r.try_get::<i64, _>("id")?) {
            continue;
        }
        let kind: String = r.try_get("kind")?;
        let Some(kind) = ConversationKind::parse(&kind) else {
            bail!("telegram_conversations.kind holds an unknown kind: {kind:?}");
        };
        let last_s: Option<i64> = r.try_get("last_ts")?;
        let deleted: Option<i8> = r.try_get("last_deleted")?;
        out.push(Conversation {
            origin: Origin::Telegram,
            id: r.try_get::<i64, _>("id")?.to_string(),
            name: r.try_get("name")?,
            kind,
            network: None,
            message_count: r.try_get("cnt")?,
            last_ts: last_s.map(s_to_ms),
            last: LastMessage::from_row(&r, deleted.unwrap_or(0) != 0)?,
            // How far I have read; no mark is unknown, not unread.
            unread: r.try_get("unread")?,
            // Filled by `avatars::attach`, which knows where the files are.
            avatar: None,
        });
    }
    Ok(out)
}

/// One page of a Telegram conversation. `sent_at` is in seconds, so `id` breaks
/// ties at every page boundary.
pub(super) async fn page(
    pool: &MySqlPool,
    conversation_id: &str,
    cursor: Option<(i64, i64)>,
    limit: i64,
    dir: PageDir,
) -> Result<Fetched> {
    // A non-numeric id matches nothing, as for any other origin.
    let Ok(conversation_id) = conversation_id.parse::<i64>() else {
        return Ok(Fetched::new(Vec::new(), Vec::new(), dir));
    };
    let (cur_ts, cur_id) = (cursor.map(|(ts, _)| ts), cursor.map(|(_, id)| id));
    let sql = match dir {
        PageDir::Older => {
            r"SELECT m.id AS id, m.msg_id AS msg_id, m.sent_at AS sent_at,
                     m.sender_name AS sender, m.is_outgoing AS is_outgoing, CAST(m.sender_id AS CHAR) AS sender_key,
                     m.text AS body, m.deleted AS deleted, m.edited_at AS edited_at,
                     m.edit_hidden AS edit_hidden,
                     m.reply_to_msg_id AS reply_to_msg_id, m.kind AS kind,
                     m.grouped_id AS grouped_id,
                     c.duration_s AS call_duration_s, c.reason AS call_reason,
                     c.video AS call_video
              FROM telegram_messages m
              LEFT JOIN telegram_calls c
                     ON c.conversation_id = m.conversation_id AND c.msg_id = m.msg_id
              WHERE m.conversation_id = ?
                AND (? IS NULL OR m.sent_at < ? OR (m.sent_at = ? AND m.id < ?))
              ORDER BY m.sent_at DESC, m.id DESC
              LIMIT ?"
        }
        PageDir::Newer => {
            r"SELECT m.id AS id, m.msg_id AS msg_id, m.sent_at AS sent_at,
                     m.sender_name AS sender, m.is_outgoing AS is_outgoing, CAST(m.sender_id AS CHAR) AS sender_key,
                     m.text AS body, m.deleted AS deleted, m.edited_at AS edited_at,
                     m.edit_hidden AS edit_hidden,
                     m.reply_to_msg_id AS reply_to_msg_id, m.kind AS kind,
                     m.grouped_id AS grouped_id,
                     c.duration_s AS call_duration_s, c.reason AS call_reason,
                     c.video AS call_video
              FROM telegram_messages m
              LEFT JOIN telegram_calls c
                     ON c.conversation_id = m.conversation_id AND c.msg_id = m.msg_id
              WHERE m.conversation_id = ?
                AND (? IS NULL OR m.sent_at > ? OR (m.sent_at = ? AND m.id > ?))
              ORDER BY m.sent_at ASC, m.id ASC
              LIMIT ?"
        }
        PageDir::AtAndNewer => {
            r"SELECT m.id AS id, m.msg_id AS msg_id, m.sent_at AS sent_at,
                     m.sender_name AS sender, m.is_outgoing AS is_outgoing, CAST(m.sender_id AS CHAR) AS sender_key,
                     m.text AS body, m.deleted AS deleted, m.edited_at AS edited_at,
                     m.edit_hidden AS edit_hidden,
                     m.reply_to_msg_id AS reply_to_msg_id, m.kind AS kind,
                     m.grouped_id AS grouped_id,
                     c.duration_s AS call_duration_s, c.reason AS call_reason,
                     c.video AS call_video
              FROM telegram_messages m
              LEFT JOIN telegram_calls c
                     ON c.conversation_id = m.conversation_id AND c.msg_id = m.msg_id
              WHERE m.conversation_id = ?
                AND (? IS NULL OR m.sent_at > ? OR (m.sent_at = ? AND m.id >= ?))
              ORDER BY m.sent_at ASC, m.id ASC
              LIMIT ?"
        }
    };
    let rows = sqlx::query(sql)
        .bind(conversation_id)
        .bind(cur_ts)
        .bind(cur_ts)
        .bind(cur_ts)
        .bind(cur_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;

    let mut msgs = Vec::with_capacity(rows.len());
    let mut keys = Vec::with_capacity(rows.len());
    let mut msg_ids = Vec::with_capacity(rows.len());
    let mut replies: Vec<(String, i32)> = Vec::new();
    for r in rows {
        let id: i64 = r.try_get("id")?;
        let sent_at: i64 = r.try_get("sent_at")?;
        if let Some(target) = r.try_get::<Option<i32>, _>("reply_to_msg_id")? {
            replies.push((id.to_string(), target));
        }
        let deleted: i8 = r.try_get("deleted")?;
        let is_outgoing: i8 = r.try_get("is_outgoing")?;
        let edited_at: Option<i64> = r.try_get("edited_at")?;
        // `edit_hide` asks for the message to be shown unmodified despite its
        // `edit_date`. NULL, on rows stored before the column, reads as not hidden.
        let edit_hidden: Option<i8> = r.try_get("edit_hidden")?;
        let edited = edited_at.is_some() && edit_hidden.unwrap_or(0) == 0;
        let is_service = r.try_get::<String, _>("kind")? == "service";
        // A call's stored text is the label "a call"; compose it from the call's
        // columns instead.
        let body = call_text(
            r.try_get("call_duration_s")?,
            r.try_get::<Option<String>, _>("call_reason")?.as_deref(),
            r.try_get::<Option<i8>, _>("call_video")?.unwrap_or(0) != 0,
        )
        .map_or_else(|| r.try_get("body"), |t| Ok(Some(t)))?;
        keys.push((sent_at, id));
        msg_ids.push(r.try_get::<i32, _>("msg_id")?);
        msgs.push(Message {
            // Service events render as actions.
            kind: if is_service {
                MessageKind::Action
            } else {
                MessageKind::Message
            },
            deleted: deleted != 0,
            edited,
            album: r
                .try_get::<Option<i64>, _>("grouped_id")?
                .map(|g| g.to_string()),
            // The row's id, unique across conversations; `msg_id` is not.
            ..Message::bare(
                id,
                s_to_ms(sent_at),
                r.try_get::<Option<String>, _>("sender")?
                    .unwrap_or_default(),
                r.try_get("sender_key")?,
                is_outgoing != 0,
                body,
            )
        });
    }

    // Telegram media as `attachments`, the one field for bytes this archive
    // holds. `available = false` is normal for large files not yet fetched.
    if !msg_ids.is_empty() {
        let placeholders = vec!["?"; msg_ids.len()].join(",");
        // The size is `telegram_messages.media_size`, from the message itself.
        let sql = format!(
            "SELECT d.msg_id AS msg_id, d.state AS state, d.content_type AS content_type,
                    m.media_size AS media_size
             FROM telegram_media d
             JOIN telegram_messages m
               ON m.conversation_id = d.conversation_id AND m.msg_id = d.msg_id
             WHERE d.conversation_id = ? AND d.msg_id IN ({placeholders})",
        );
        let mut q = sqlx::query(AssertSqlSafe(sql)).bind(conversation_id);
        for id in &msg_ids {
            q = q.bind(id);
        }
        for mr in q.fetch_all(pool).await? {
            let msg_id: i32 = mr.try_get("msg_id")?;
            let state: String = mr.try_get("state")?;
            let content_type: Option<String> = mr.try_get("content_type")?;
            let Some(i) = msg_ids.iter().position(|m| *m == msg_id) else {
                continue;
            };
            let api_id = msgs[i].id.clone();
            msgs[i].attachments.push(Attachment {
                // The message's API id, which is what the media route takes.
                id: api_id,
                is_image: is_image(content_type.as_deref()),
                content_type,
                // Telegram photos carry no filename; the frontend names them.
                file_name: None,
                size: mr.try_get("media_size")?,
                available: state == "stored",
                fetch: FetchState::parse(&state),
            });
        }
    }

    // Reactions, current only (`removed_at IS NULL`). A custom emoji has no
    // characters to draw, so its rows are left out.
    if !msg_ids.is_empty() {
        let placeholders = vec!["?"; msg_ids.len()].join(",");
        let sql = format!(
            "SELECT msg_id, emoji, cnt FROM telegram_reactions
             WHERE conversation_id = ? AND emoji IS NOT NULL
               AND removed_at IS NULL
               AND msg_id IN ({placeholders})",
        );
        let mut q = sqlx::query(AssertSqlSafe(sql)).bind(conversation_id);
        for id in &msg_ids {
            q = q.bind(id);
        }
        let named = reactors(pool, conversation_id, &msg_ids).await?;
        for rr in q.fetch_all(pool).await? {
            let msg_id: i32 = rr.try_get("msg_id")?;
            let emoji: String = rr.try_get("emoji")?;
            let reaction = Reaction {
                count: i64::from(rr.try_get::<i32, _>("cnt")?),
                who: named
                    .get(&(msg_id, emoji.clone()))
                    .cloned()
                    .unwrap_or_default(),
                emoji,
            };
            // Positioned by `msg_id`, not the API id.
            if let Some(i) = msg_ids.iter().position(|m| *m == msg_id) {
                msgs[i].reactions.push(reaction);
            }
        }
    }

    attach_replies(pool, conversation_id, &mut msgs, &replies).await?;
    attach_read(pool, conversation_id, &mut msgs, &msg_ids).await?;
    attach_entities(pool, conversation_id, &mut msgs, &msg_ids).await?;

    Ok(Fetched::new(msgs, keys, dir))
}

/// Resolve Telegram's replies for one page. Service events resolve too, since
/// the page returns them: an id in a [`ReplyTo`] must be somewhere the reader can
/// be taken.
async fn attach_replies(
    pool: &MySqlPool,
    conversation_id: i64,
    msgs: &mut [Message],
    replies: &[(String, i32)],
) -> Result<()> {
    if replies.is_empty() {
        return Ok(());
    }
    let targets = distinct(replies.iter().map(|(_, m)| *m));
    let placeholders = vec!["?"; targets.len()].join(",");
    let sql = format!(
        "SELECT m.id AS id, m.msg_id AS msg_id, m.sent_at AS sent_at, m.text AS body,
                m.deleted AS deleted, m.sender_name AS sender,
                c.duration_s AS call_duration_s, c.reason AS call_reason,
                c.video AS call_video
         FROM telegram_messages m
         LEFT JOIN telegram_calls c
                ON c.conversation_id = m.conversation_id AND c.msg_id = m.msg_id
         WHERE m.conversation_id = ?
           AND m.msg_id IN ({placeholders})",
    );
    let mut q = sqlx::query(AssertSqlSafe(sql)).bind(conversation_id);
    for id in &targets {
        q = q.bind(id);
    }
    let mut found: HashMap<i32, ReplyTo> = HashMap::new();
    for r in q.fetch_all(pool).await? {
        let id: i64 = r.try_get("id")?;
        let msg_id: i32 = r.try_get("msg_id")?;
        let sent_at: i64 = r.try_get("sent_at")?;
        let deleted: i8 = r.try_get("deleted")?;
        let deleted = deleted != 0;
        // Composed as the page composes it, so a reply quotes what the call
        // message shows.
        let body: Option<String> = call_text(
            r.try_get("call_duration_s")?,
            r.try_get::<Option<String>, _>("call_reason")?.as_deref(),
            r.try_get::<Option<i8>, _>("call_video")?.unwrap_or(0) != 0,
        )
        .map_or_else(|| r.try_get("body"), |t| Ok(Some(t)))?;
        found.insert(
            msg_id,
            ReplyTo {
                id: Some(id.to_string()),
                // The cursor in Telegram's seconds; `ts` in milliseconds.
                cursor: Some(encode_cursor(sent_at, id)),
                ts: Some(s_to_ms(sent_at)),
                sender: r.try_get::<Option<String>, _>("sender").ok().flatten(),
                excerpt: if deleted {
                    None
                } else {
                    excerpt(body.as_deref())
                },
                deleted,
            },
        );
    }
    for (msg_id, target) in replies {
        let Some(m) = msgs.iter_mut().find(|m| &m.id == msg_id) else {
            continue;
        };
        // A Telegram reply names an id, which carries no time.
        m.reply_to = Some(found.get(target).cloned().unwrap_or_else(ReplyTo::not_held));
    }
    Ok(())
}

/// Who reacted to each of these messages, by `(msg_id, emoji)`. A reactor is
/// named by their conversation, else by any message they sent (the self user has
/// no conversation row), else by their id as text.
async fn reactors(
    pool: &MySqlPool,
    conversation_id: i64,
    msg_ids: &[i32],
) -> Result<HashMap<(i32, String), Vec<String>>> {
    let mut out: HashMap<(i32, String), Vec<String>> = HashMap::new();
    if msg_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = vec!["?"; msg_ids.len()].join(",");
    // Current reactions only.
    let sql = format!(
        "SELECT a.msg_id, a.emoji, a.peer_id,
                COALESCE(c.name, s.sender_name, CAST(a.peer_id AS CHAR)) AS who
           FROM telegram_reaction_authors a
           LEFT JOIN telegram_conversations c ON c.id = a.peer_id
           LEFT JOIN (SELECT sender_id, MIN(sender_name) AS sender_name
                        FROM telegram_messages
                       WHERE sender_id IS NOT NULL AND sender_name IS NOT NULL
                       GROUP BY sender_id) s ON s.sender_id = a.peer_id
          WHERE a.conversation_id = ? AND a.removed_at IS NULL
            AND a.emoji IS NOT NULL
            AND a.msg_id IN ({placeholders})
          ORDER BY a.reacted_at, who",
    );
    let mut q = sqlx::query(AssertSqlSafe(sql)).bind(conversation_id);
    for id in msg_ids {
        q = q.bind(id);
    }
    for r in q.fetch_all(pool).await? {
        let key = (r.try_get("msg_id")?, r.try_get("emoji")?);
        let who: String = r.try_get("who")?;
        let names = out.entry(key).or_default();
        if !names.contains(&who) {
            names.push(who);
        }
    }
    Ok(out)
}

/// Telegram read state for this page's outgoing messages. The mark is one
/// high-water `msg_id` per conversation (`outbox`: how far they have read mine),
/// so this is a comparison. No mark leaves every message `None`.
async fn attach_read(
    pool: &MySqlPool,
    conversation_id: i64,
    msgs: &mut [Message],
    msg_ids: &[i32],
) -> Result<()> {
    let mark: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(max_id) FROM telegram_read_marks
          WHERE conversation_id = ? AND direction = 'outbox'",
    )
    .bind(conversation_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    let Some(mark) = mark else {
        return Ok(());
    };
    for (i, msg_id) in msg_ids.iter().enumerate() {
        if msgs[i].is_outgoing {
            let state = if *msg_id <= mark {
                DeliveryState::Read
            } else {
                DeliveryState::Sent
            };
            // Telegram names nobody who read.
            msgs[i].delivery = Some(Delivery {
                state,
                read_by: Vec::new(),
            });
        }
    }
    Ok(())
}

/// The formatting runs on this page's messages, current only (an edit dates the
/// old ones), ordered by offset for the reader's single pass.
async fn attach_entities(
    pool: &MySqlPool,
    conversation_id: i64,
    msgs: &mut [Message],
    msg_ids: &[i32],
) -> Result<()> {
    if msg_ids.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; msg_ids.len()].join(",");
    let sql = format!(
        "SELECT msg_id, kind, offset_utf16, length_utf16, url
           FROM telegram_message_entities
          WHERE conversation_id = ? AND removed_at IS NULL
            AND msg_id IN ({placeholders})
          ORDER BY msg_id, offset_utf16, length_utf16",
    );
    let mut q = sqlx::query(AssertSqlSafe(sql)).bind(conversation_id);
    for id in msg_ids {
        q = q.bind(id);
    }
    for row in q.fetch_all(pool).await? {
        let msg_id: i32 = row.try_get("msg_id")?;
        let Some(i) = msg_ids.iter().position(|m| *m == msg_id) else {
            continue;
        };
        let offset: i32 = row.try_get("offset_utf16")?;
        let length: i32 = row.try_get("length_utf16")?;
        msgs[i].entities.push(Entity {
            kind: row.try_get("kind")?,
            offset: offset.into(),
            length: length.into(),
            url: row.try_get("url")?,
        });
    }
    Ok(())
}

/// Telegram's edit history. Each superseded version is filed under the
/// `edit_date` it carried; the original carried none, so NULL sorts first.
/// `m.edited` already honours `edit_hide`, which hides the history too.
pub(super) async fn attach_edits(
    pool: &MySqlPool,
    conversation_id: &str,
    msgs: &mut [Message],
) -> Result<()> {
    let Ok(conversation_id) = conversation_id.parse::<i64>() else {
        return Ok(());
    };
    let ids: Vec<i64> = msgs
        .iter()
        .filter(|m| m.edited)
        .filter_map(|m| m.id.parse::<i64>().ok())
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT m.id AS row_id, e.was_edited_at AS was_edited_at, e.text AS text,
                m.sent_at AS sent_at
         FROM telegram_message_edits e
         JOIN telegram_messages m
           ON m.conversation_id = e.conversation_id AND m.msg_id = e.msg_id
         WHERE e.conversation_id = ? AND m.id IN ({placeholders})
         ORDER BY e.was_edited_at IS NOT NULL, e.was_edited_at ASC, e.id ASC",
    );
    let mut q = sqlx::query(AssertSqlSafe(sql)).bind(conversation_id);
    for id in &ids {
        q = q.bind(id);
    }
    for row in q.fetch_all(pool).await? {
        let row_id: i64 = row.try_get("row_id")?;
        let row_id = row_id.to_string();
        // The original is dated by the message; later versions by the edit date
        // they carried.
        let was: Option<i64> = row.try_get("was_edited_at")?;
        let sent_at: i64 = row.try_get("sent_at")?;
        let edit = MessageEdit {
            ts: s_to_ms(was.unwrap_or(sent_at)),
            body: row.try_get("text")?,
        };
        if let Some(m) = msgs.iter_mut().find(|m| m.id == row_id) {
            m.edits.push(edit);
        }
    }
    Ok(())
}

/// What the archive now holds for one Telegram message, for a reader polling a
/// fetch it asked for: the fetch happens later, in another process.
pub async fn media_state(pool: &MySqlPool, row_id: i64) -> Result<Option<MediaState>> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT d.state, d.content_type
           FROM telegram_messages m
           JOIN telegram_media d
             ON d.conversation_id = m.conversation_id AND d.msg_id = m.msg_id
          WHERE m.id = ?",
    )
    .bind(row_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(state, content_type)| MediaState {
        available: state == "stored",
        fetch: FetchState::parse(&state),
        content_type,
    }))
}

/// A reader asked for a Telegram file the archive has not fetched. Only
/// `offered` and `failed` rows are queued. Returns whether anything changed.
pub async fn request_media(pool: &MySqlPool, row_id: i64) -> Result<bool> {
    let changed = sqlx::query(
        "UPDATE telegram_media d
           JOIN telegram_messages m
             ON m.conversation_id = d.conversation_id AND m.msg_id = d.msg_id
            SET d.state = 'wanted', d.requested_at = CURRENT_TIMESTAMP
          WHERE m.id = ? AND d.state IN ('offered', 'failed')",
    )
    .bind(row_id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(changed != 0)
}

/// The bytes held for a Telegram message, by the message's API id. The join
/// keeps a request inside the message's own conversation.
pub async fn media_blob(pool: &MySqlPool, row_id: i64) -> Result<Option<(Option<String>, String)>> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT d.content_type, d.stored_name
           FROM telegram_messages m
           JOIN telegram_media d
             ON d.conversation_id = m.conversation_id AND d.msg_id = m.msg_id
          WHERE m.id = ? AND d.state = 'stored'",
    )
    .bind(row_id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some((ct, Some(name))) => Some((ct, name)),
        _ => None,
    })
}

/// Telegram messages whose text matches `like`, in one conversation when `id` is given.
pub(super) async fn search(
    pool: &MySqlPool,
    like: &str,
    id: Option<&str>,
    limit: i64,
) -> Result<Vec<SearchHit>> {
    let mut hits = Vec::new();
    // Only what was said, not service events.
    let sql = if id.is_some() {
        r"SELECT m.id AS id, m.conversation_id AS cid, t.name AS cname,
                 m.sent_at AS sent_at, m.sender_name AS sender, m.text AS body,
                 m.deleted AS deleted
          FROM telegram_messages m
          LEFT JOIN telegram_conversations t ON t.id = m.conversation_id
          WHERE m.kind = 'message' AND m.text LIKE ? AND m.conversation_id = ?
          ORDER BY m.sent_at DESC LIMIT ?"
    } else {
        r"SELECT m.id AS id, m.conversation_id AS cid, t.name AS cname,
                 m.sent_at AS sent_at, m.sender_name AS sender, m.text AS body,
                 m.deleted AS deleted
          FROM telegram_messages m
          LEFT JOIN telegram_conversations t ON t.id = m.conversation_id
          WHERE m.kind = 'message' AND m.text LIKE ?
          ORDER BY m.sent_at DESC LIMIT ?"
    };
    let mut q = sqlx::query(sql).bind(like);
    // Bound as a string against a BIGINT: a malformed id matches nothing
    // rather than failing.
    if let Some(id) = id {
        q = q.bind(id);
    }
    let rows = q.bind(limit).fetch_all(pool).await?;
    for r in rows {
        let sent_at: i64 = r.try_get("sent_at")?;
        let id: i64 = r.try_get("id")?;
        let deleted: i8 = r.try_get("deleted")?;
        hits.push(SearchHit {
            origin: Origin::Telegram,
            conversation_id: r.try_get::<i64, _>("cid")?.to_string(),
            conversation_name: r.try_get("cname")?,
            ts: s_to_ms(sent_at),
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            snippet: r.try_get::<Option<String>, _>("body")?.unwrap_or_default(),
            deleted: deleted != 0,
            // Seconds, as `telegram_messages` pages on.
            cursor: encode_cursor(sent_at, id),
        });
    }
    Ok(hits)
}
