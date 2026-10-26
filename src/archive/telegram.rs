//! Telegram: `telegram_*`, with media fetched on request, reactions, read marks,
//! formatting entities and edit history.

use std::collections::HashMap;

use anyhow::{Result, bail};
use sqlx::{AssertSqlSafe, MySqlPool, Row};

use super::*;

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
                        AND u.kind = 'message' AND u.is_outgoing = 0
                        AND u.deleted = 0) END AS unread
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
        let deleted: Option<bool> = r.try_get("last_deleted")?;
        out.push(Conversation {
            origin: Origin::Telegram,
            id: r.try_get::<i64, _>("id")?.to_string(),
            name: r.try_get("name")?,
            kind,
            network: None,
            message_count: r.try_get("cnt")?,
            last_ts: last_s.map(s_to_ms),
            last: LastMessage::from_row(&r, deleted.unwrap_or(false))?,
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
                     COALESCE((SELECT MAX(NOT (e.text <=> m.text)) FROM telegram_message_edits e
                                WHERE e.conversation_id = m.conversation_id AND e.msg_id = m.msg_id),
                              NOT (m.media_kind <=> 'geo')
                              AND NOT EXISTS (SELECT 1 FROM telegram_reaction_authors r
                                               WHERE r.conversation_id = m.conversation_id
                                                 AND r.msg_id = m.msg_id
                                                 AND ABS(r.reacted_at - m.edited_at) <= 2))
                       AS reworded,
                     m.reply_to_msg_id AS reply_to_msg_id,
                     m.reply_to_peer_id AS reply_to_peer_id, m.kind AS kind,
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
                     COALESCE((SELECT MAX(NOT (e.text <=> m.text)) FROM telegram_message_edits e
                                WHERE e.conversation_id = m.conversation_id AND e.msg_id = m.msg_id),
                              NOT (m.media_kind <=> 'geo')
                              AND NOT EXISTS (SELECT 1 FROM telegram_reaction_authors r
                                               WHERE r.conversation_id = m.conversation_id
                                                 AND r.msg_id = m.msg_id
                                                 AND ABS(r.reacted_at - m.edited_at) <= 2))
                       AS reworded,
                     m.reply_to_msg_id AS reply_to_msg_id,
                     m.reply_to_peer_id AS reply_to_peer_id, m.kind AS kind,
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
                     COALESCE((SELECT MAX(NOT (e.text <=> m.text)) FROM telegram_message_edits e
                                WHERE e.conversation_id = m.conversation_id AND e.msg_id = m.msg_id),
                              NOT (m.media_kind <=> 'geo')
                              AND NOT EXISTS (SELECT 1 FROM telegram_reaction_authors r
                                               WHERE r.conversation_id = m.conversation_id
                                                 AND r.msg_id = m.msg_id
                                                 AND ABS(r.reacted_at - m.edited_at) <= 2))
                       AS reworded,
                     m.reply_to_msg_id AS reply_to_msg_id,
                     m.reply_to_peer_id AS reply_to_peer_id, m.kind AS kind,
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
    let mut replies: Vec<(i64, i32)> = Vec::new();
    let mut msg_index: HashMap<i32, usize> = HashMap::new();
    for r in rows {
        let id: i64 = r.try_get("id")?;
        let sent_at: i64 = r.try_get("sent_at")?;
        // A reply into another chat names a number that means nothing here; it
        // stays a reply, to a message this conversation does not hold.
        let elsewhere = r
            .try_get::<Option<i64>, _>("reply_to_peer_id")?
            .is_some_and(|p| p != conversation_id);
        let reply_target: Option<i32> = r.try_get("reply_to_msg_id")?;
        if let Some(target) = reply_target.filter(|_| !elsewhere) {
            replies.push((id, target));
        }
        let edited_at: Option<i64> = r.try_get("edited_at")?;
        // `edit_hide` asks for the message to be shown unmodified despite its
        // `edit_date`. NULL, on rows stored before the column, reads as not hidden.
        let edit_hidden: Option<bool> = r.try_get("edit_hidden")?;
        // Telegram moves `edit_date` for a reaction and for a live location's
        // move, as well as for an edit. With a history, an edit is a version whose
        // words differ. Without one, a reaction within 2 s of the date or a live
        // location explains it; otherwise it was edited before the archive saw it.
        let reworded: bool = r.try_get("reworded")?;
        let is_service = r.try_get::<String, _>("kind")? == "service";
        keys.push((sent_at, id));
        msgs.push(Message {
            reply_to: (elsewhere && reply_target.is_some()).then(ReplyTo::not_held),
            // Service events render as actions.
            kind: if is_service {
                MessageKind::Action
            } else {
                MessageKind::Message
            },
            deleted: r.try_get("deleted")?,
            edited: edited_at.is_some() && edit_hidden != Some(true) && reworded,
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
                r.try_get("is_outgoing")?,
                stored_or_call_text(&r)?,
            )
        });
        // Positioned by `msg_id`, which is unique only within the conversation.
        msg_index.insert(r.try_get::<i32, _>("msg_id")?, msgs.len() - 1);
    }
    let msg_ids: Vec<i32> = msg_index.keys().copied().collect();

    attach_media(pool, conversation_id, &mut msgs, &msg_index, &msg_ids).await?;
    attach_reactions(pool, conversation_id, &mut msgs, &msg_index, &msg_ids).await?;
    attach_replies(pool, conversation_id, &mut msgs, &replies).await?;
    attach_read(pool, conversation_id, &mut msgs, &msg_index).await?;
    attach_entities(pool, conversation_id, &mut msgs, &msg_index, &msg_ids).await?;

    Ok(Fetched::new(msgs, keys, dir))
}

/// A row's text, or for a call, the call in words: its stored text is the
/// label "a call".
fn stored_or_call_text(r: &sqlx::mysql::MySqlRow) -> Result<Option<String>> {
    let call = call_text(
        r.try_get("call_duration_s")?,
        r.try_get::<Option<String>, _>("call_reason")?.as_deref(),
        r.try_get::<Option<bool>, _>("call_video")?.unwrap_or(false),
    );
    Ok(match call {
        Some(words) => Some(words),
        None => r.try_get("body")?,
    })
}

/// What `telegram_media.state` says about a file: held, or how a reader may get it.
fn media_state_of(state: &str) -> (bool, Option<FetchState>) {
    (state == "stored", FetchState::parse(state))
}

/// Telegram media as `attachments`, the one field for bytes this archive
/// holds. `available = false` is normal for large files not yet fetched.
async fn attach_media(
    pool: &MySqlPool,
    conversation_id: i64,
    msgs: &mut [Message],
    msg_index: &HashMap<i32, usize>,
    msg_ids: &[i32],
) -> Result<()> {
    if msg_ids.is_empty() {
        return Ok(());
    }
    // The size is `telegram_messages.media_size`, from the message itself.
    let sql = in_list(
        "SELECT d.msg_id AS msg_id, d.state AS state, d.content_type AS content_type,
                m.media_size AS media_size
         FROM telegram_media d
         JOIN telegram_messages m
           ON m.conversation_id = d.conversation_id AND m.msg_id = d.msg_id
         WHERE d.conversation_id = ? AND d.msg_id IN ({in})",
        msg_ids.len(),
    );
    for r in bind_all(
        sqlx::query(AssertSqlSafe(sql)).bind(conversation_id),
        msg_ids,
    )
    .fetch_all(pool)
    .await?
    {
        let Some(&i) = msg_index.get(&r.try_get::<i32, _>("msg_id")?) else {
            continue;
        };
        let (available, fetch) = media_state_of(&r.try_get::<String, _>("state")?);
        let id = msgs[i].id;
        msgs[i].attachments.push(Attachment {
            // The message's API id, which is what the media route takes.
            id,
            content_type: r.try_get("content_type")?,
            // Telegram photos carry no filename; the frontend names them.
            file_name: None,
            size: r.try_get("media_size")?,
            available,
            fetch,
            // Filled by `attach_transcripts`.
            transcript: None,
        });
    }
    Ok(())
}

/// Reactions, current only (`removed_at IS NULL`). A custom emoji has no
/// characters to draw, so its rows are left out.
async fn attach_reactions(
    pool: &MySqlPool,
    conversation_id: i64,
    msgs: &mut [Message],
    msg_index: &HashMap<i32, usize>,
    msg_ids: &[i32],
) -> Result<()> {
    if msg_ids.is_empty() {
        return Ok(());
    }
    let sql = in_list(
        "SELECT msg_id, emoji, cnt FROM telegram_reactions
         WHERE conversation_id = ? AND emoji IS NOT NULL
           AND removed_at IS NULL
           AND msg_id IN ({in})",
        msg_ids.len(),
    );
    let rows = bind_all(
        sqlx::query(AssertSqlSafe(sql)).bind(conversation_id),
        msg_ids,
    )
    .fetch_all(pool)
    .await?;
    let mut named = reactors(pool, conversation_id, msg_ids).await?;
    for r in rows {
        let msg_id: i32 = r.try_get("msg_id")?;
        let Some(&i) = msg_index.get(&msg_id) else {
            continue;
        };
        let emoji: String = r.try_get("emoji")?;
        msgs[i].reactions.push(Reaction {
            count: r.try_get::<i32, _>("cnt")?.into(),
            who: named.remove(&(msg_id, emoji.clone())).unwrap_or_default(),
            emoji,
        });
    }
    Ok(())
}

/// Resolve Telegram's replies for one page. Service events resolve too, since
/// the page returns them: an id in a [`ReplyTo`] must be somewhere the reader can
/// be taken.
async fn attach_replies(
    pool: &MySqlPool,
    conversation_id: i64,
    msgs: &mut [Message],
    replies: &[(i64, i32)],
) -> Result<()> {
    if replies.is_empty() {
        return Ok(());
    }
    let targets = distinct(replies.iter().map(|(_, m)| *m));
    let sql = in_list(
        "SELECT m.id AS id, m.msg_id AS msg_id, m.sent_at AS sent_at, m.text AS body,
                m.deleted AS deleted, m.sender_name AS sender,
                c.duration_s AS call_duration_s, c.reason AS call_reason,
                c.video AS call_video
         FROM telegram_messages m
         LEFT JOIN telegram_calls c
                ON c.conversation_id = m.conversation_id AND c.msg_id = m.msg_id
         WHERE m.conversation_id = ?
           AND m.msg_id IN ({in})",
        targets.len(),
    );
    let mut found: HashMap<i32, ReplyTo> = HashMap::new();
    for r in bind_all(
        sqlx::query(AssertSqlSafe(sql)).bind(conversation_id),
        &targets,
    )
    .fetch_all(pool)
    .await?
    {
        let id: i64 = r.try_get("id")?;
        let sent_at: i64 = r.try_get("sent_at")?;
        let deleted: bool = r.try_get("deleted")?;
        // Composed as the page composes it, so a reply quotes what the call
        // message shows.
        let body = stored_or_call_text(&r)?;
        found.insert(
            r.try_get("msg_id")?,
            ReplyTo {
                id: Some(id),
                // The cursor in Telegram's seconds; `ts` in milliseconds.
                cursor: Some(encode_cursor(sent_at, id)),
                ts: Some(s_to_ms(sent_at)),
                sender: r.try_get("sender")?,
                excerpt: if deleted {
                    None
                } else {
                    excerpt(body.as_deref())
                },
                deleted,
            },
        );
    }
    let mut by = by_row(msgs);
    for (msg_id, target) in replies {
        let Some(m) = by.get_mut(msg_id) else {
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
    let sql = in_list(
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
            AND a.msg_id IN ({in})
          ORDER BY a.reacted_at, who",
        msg_ids.len(),
    );
    for r in bind_all(
        sqlx::query(AssertSqlSafe(sql)).bind(conversation_id),
        msg_ids,
    )
    .fetch_all(pool)
    .await?
    {
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
    msg_index: &HashMap<i32, usize>,
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
    for (&msg_id, &i) in msg_index {
        if msgs[i].is_outgoing {
            msgs[i].delivery = Some(Delivery {
                state: if msg_id <= mark {
                    DeliveryState::Read
                } else {
                    DeliveryState::Sent
                },
                // Telegram names nobody who read.
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
    msg_index: &HashMap<i32, usize>,
    msg_ids: &[i32],
) -> Result<()> {
    if msg_ids.is_empty() {
        return Ok(());
    }
    let sql = in_list(
        "SELECT msg_id, kind, offset_utf16, length_utf16, url
           FROM telegram_message_entities
          WHERE conversation_id = ? AND removed_at IS NULL
            AND msg_id IN ({in})
          ORDER BY msg_id, offset_utf16, length_utf16",
        msg_ids.len(),
    );
    for r in bind_all(
        sqlx::query(AssertSqlSafe(sql)).bind(conversation_id),
        msg_ids,
    )
    .fetch_all(pool)
    .await?
    {
        let Some(&i) = msg_index.get(&r.try_get::<i32, _>("msg_id")?) else {
            continue;
        };
        msgs[i].entities.push(Entity {
            kind: r.try_get("kind")?,
            offset: r.try_get::<i32, _>("offset_utf16")?.into(),
            length: r.try_get::<i32, _>("length_utf16")?.into(),
            url: r.try_get("url")?,
        });
    }
    Ok(())
}

/// Telegram's edit history. Each superseded version is filed under the
/// `edit_date` it carried; the original carried none, so NULL sorts first.
/// `m.edited` already honours `edit_hide`, which hides the history too. A
/// version that reads as the one after it is left out: a reaction moved the
/// `edit_date`, and older rows filed the unchanged words.
pub(super) async fn attach_edits(
    pool: &MySqlPool,
    conversation_id: &str,
    msgs: &mut [Message],
) -> Result<()> {
    let Ok(conversation_id) = conversation_id.parse::<i64>() else {
        return Ok(());
    };
    let ids: Vec<i64> = msgs.iter().filter(|m| m.edited).map(|m| m.id).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let sql = in_list(
        "SELECT m.id AS row_id, e.was_edited_at AS was_edited_at, e.text AS text,
                m.sent_at AS sent_at
         FROM telegram_message_edits e
         JOIN telegram_messages m
           ON m.conversation_id = e.conversation_id AND m.msg_id = e.msg_id
         WHERE e.conversation_id = ? AND m.id IN ({in})
         ORDER BY e.was_edited_at IS NOT NULL, e.was_edited_at ASC, e.id ASC",
        ids.len(),
    );
    let rows = bind_all(sqlx::query(AssertSqlSafe(sql)).bind(conversation_id), &ids)
        .fetch_all(pool)
        .await?;
    let mut by = by_row(msgs);
    for r in rows {
        let Some(m) = by.get_mut(&r.try_get::<i64, _>("row_id")?) else {
            continue;
        };
        // The original is dated by the message; later versions by the edit date
        // they carried.
        let was: Option<i64> = r.try_get("was_edited_at")?;
        m.edits.push(MessageEdit {
            ts: s_to_ms(was.unwrap_or(r.try_get("sent_at")?)),
            body: r.try_get("text")?,
        });
    }
    // Newest first: each version against the one that replaced it, the newest
    // against the text itself.
    for m in by.into_values().filter(|m| !m.edits.is_empty()) {
        let mut next = m.body.clone();
        let mut kept: Vec<MessageEdit> = Vec::with_capacity(m.edits.len());
        for e in m.edits.drain(..).rev() {
            if e.body != next {
                next.clone_from(&e.body);
                kept.push(e);
            }
        }
        kept.reverse();
        m.edits = kept;
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
    Ok(row.map(|(state, content_type)| {
        let (available, fetch) = media_state_of(&state);
        MediaState {
            available,
            fetch,
            content_type,
        }
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
        hits.push(SearchHit {
            origin: Origin::Telegram,
            conversation_id: r.try_get::<i64, _>("cid")?.to_string(),
            conversation_name: r.try_get("cname")?,
            ts: s_to_ms(sent_at),
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            snippet: r.try_get::<Option<String>, _>("body")?.unwrap_or_default(),
            deleted: r.try_get("deleted")?,
            // Seconds, as `telegram_messages` pages on.
            cursor: encode_cursor(sent_at, id),
        });
    }
    Ok(hits)
}
