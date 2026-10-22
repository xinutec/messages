//! Signal: `messages`, with its quotes, edits, styles, previews, reactions,
//! attachments and receipts.

use std::collections::HashMap;

use anyhow::{Result, bail};
use sqlx::{AssertSqlSafe, MySqlPool, Row};

use super::*;

/// A group call, as Signal's own thread names it. Every participant's phone
/// sends an update on joining and on leaving, all under the call's era; the
/// first one held in the thread stands for the call, and the rest are left out
/// wherever messages are shown or counted.
const GROUP_CALL: &str = "started a group call";

pub(super) async fn conversations(pool: &MySqlPool) -> Result<Vec<Conversation>> {
    let mut out = Vec::new();
    // An edit's revision rows are versions, not messages: the page leaves them
    // out, so the count, the last message and its time do too. The last message
    // reads as the page shows it, in its newest revision's words.
    // A group call's later updates are left out the same way ([`GROUP_CALL`]).
    //
    // Unread: theirs after my newest read, as the phone counts them, so not one
    // deleted for everyone (the list still says so). A read receipt on a message someone
    // else sent can only be mine, synced from the phone. Before the first receipt
    // was captured nothing can be told, so nothing older counts.
    let signal = sqlx::query(
        r"SELECT c.thread_id AS id, c.type AS kind, c.name AS name,
                 COALESCE(s.cnt, 0) AS cnt, s.last_msg_ts AS last_ts,
                 COALESCE(rv.id, l.id) AS last_id, l.deleted AS last_deleted, l.is_outgoing AS last_out,
                 CASE WHEN rv.id IS NULL THEN l.body ELSE rv.body END AS last_text,
                 COALESCE(ct.display_name, l.sender_uuid) AS last_sender,
                 l.group_call_era AS last_call,
                 (SELECT a.content_type FROM attachments a
                   WHERE a.message_id = l.id ORDER BY a.id LIMIT 1) AS last_media,
                 (SELECT COUNT(*) FROM messages u
                   WHERE u.thread_id = c.thread_id AND u.is_outgoing = 0
                     AND u.edit_of_ts IS NULL AND u.deleted = 0
                     AND (u.group_call_era IS NULL OR NOT EXISTS (
                         SELECT 1 FROM messages g
                          WHERE g.thread_id = u.thread_id AND g.group_call_era = u.group_call_era
                            AND (g.server_ts < u.server_ts OR (g.server_ts = u.server_ts AND g.id < u.id))))
                     AND u.server_ts > GREATEST(COALESCE(rd.read_ts, 0), rs.since_ms)) AS unread
          FROM conversations c
          LEFT JOIN (
              SELECT m.thread_id, COUNT(CASE WHEN m.edit_of_ts IS NULL THEN m.id END) AS cnt,
                     MAX(CASE WHEN m.edit_of_ts IS NULL THEN m.server_ts END) AS last_msg_ts
              FROM messages m
              WHERE (m.group_call_era IS NULL OR NOT EXISTS (
                       SELECT 1 FROM messages g
                        WHERE g.thread_id = m.thread_id AND g.group_call_era = m.group_call_era
                          AND (g.server_ts < m.server_ts OR (g.server_ts = m.server_ts AND g.id < m.id))))
              GROUP BY m.thread_id
          ) s ON s.thread_id = c.thread_id
          LEFT JOIN messages l
                 ON l.thread_id = c.thread_id AND l.server_ts = s.last_msg_ts
                AND l.edit_of_ts IS NULL
          LEFT JOIN messages rv
                 ON rv.id = (SELECT r.id FROM messages r
                              WHERE r.thread_id = l.thread_id AND r.sender_uuid = l.sender_uuid
                                AND r.edit_of_ts = l.server_ts
                              ORDER BY r.server_ts DESC, r.id DESC LIMIT 1)
          LEFT JOIN contacts ct ON ct.uuid = l.sender_uuid
          LEFT JOIN (
              SELECT m.thread_id, MAX(m.server_ts) AS read_ts
              FROM signal_receipts r
              JOIN messages m ON m.server_ts = r.target_ts AND m.is_outgoing = 0
              WHERE r.kind = 'read'
              GROUP BY m.thread_id
          ) rd ON rd.thread_id = c.thread_id
          CROSS JOIN (
              SELECT COALESCE(UNIX_TIMESTAMP(MIN(observed_at)) * 1000, ~0 >> 1) AS since_ms
              FROM signal_receipts
          ) rs
          ORDER BY l.id DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut seen = std::collections::HashSet::new();
    let mut bodies: HashMap<i64, String> = HashMap::new();
    let mut last_rows = Vec::new();
    for r in signal {
        let id: String = r.try_get("id")?;
        if !seen.insert(id.clone()) {
            continue;
        }
        let kind: String = r.try_get("kind")?;
        let Some(kind) = ConversationKind::parse(&kind) else {
            bail!("conversations.type holds an unknown kind: {kind:?}");
        };
        let deleted: Option<bool> = r.try_get("last_deleted")?;
        let last_id: Option<i64> = r.try_get("last_id")?;
        let mut last = LastMessage::from_row(&r, deleted.unwrap_or(false))?;
        if let Some(l) = last.as_mut().filter(|l| l.text.is_none() && !l.deleted)
            && r.try_get::<Option<String>, _>("last_call")?.is_some()
        {
            l.text = Some(GROUP_CALL.to_owned());
        }
        if let (Some(row), Some(text)) = (last_id, r.try_get::<Option<String>, _>("last_text")?) {
            bodies.insert(row, text);
            last_rows.push((out.len(), row));
        }
        out.push(Conversation {
            origin: Origin::Signal,
            id,
            name: r.try_get("name")?,
            kind,
            network: None,
            message_count: r.try_get("cnt")?,
            last_ts: r.try_get("last_ts")?,
            last,
            unread: r.try_get("unread")?,
            // Filled by `avatars::attach`, which knows where the files are.
            avatar: None,
        });
    }
    // Mentions named, as in the thread.
    with_names(pool, &mut bodies).await?;
    for (i, row) in last_rows {
        if let Some(last) = out[i].last.as_mut().filter(|l| l.text.is_some()) {
            last.text = excerpt(bodies.get(&row).map(String::as_str));
        }
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
                 COALESCE(ct.display_name, m.sender_uuid) AS sender, m.sender_uuid AS sender_key,
                 m.is_outgoing AS is_outgoing, m.body AS body,
                 m.deleted AS deleted, m.edited AS edited,
                 m.quote_target_ts AS quote_target_ts, m.quote_text AS quote_text,
                 COALESCE(qa.display_name, m.quote_author_uuid) AS quote_author,
                 m.group_call_era AS group_call_era
          FROM messages m
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          LEFT JOIN contacts qa ON qa.uuid = m.quote_author_uuid
          WHERE m.thread_id = ?
            AND m.edit_of_ts IS NULL
            AND (m.group_call_era IS NULL OR NOT EXISTS (
                     SELECT 1 FROM messages g
                      WHERE g.thread_id = m.thread_id AND g.group_call_era = m.group_call_era
                        AND (g.server_ts < m.server_ts OR (g.server_ts = m.server_ts AND g.id < m.id))))
            AND (? IS NULL OR m.server_ts < ? OR (m.server_ts = ? AND m.id < ?))
          ORDER BY m.server_ts DESC, m.id DESC
          LIMIT ?"
        }
        PageDir::Newer => {
            r"SELECT m.id AS id, m.server_ts AS ts,
                 COALESCE(ct.display_name, m.sender_uuid) AS sender, m.sender_uuid AS sender_key,
                 m.is_outgoing AS is_outgoing, m.body AS body,
                 m.deleted AS deleted, m.edited AS edited,
                 m.quote_target_ts AS quote_target_ts, m.quote_text AS quote_text,
                 COALESCE(qa.display_name, m.quote_author_uuid) AS quote_author,
                 m.group_call_era AS group_call_era
          FROM messages m
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          LEFT JOIN contacts qa ON qa.uuid = m.quote_author_uuid
          WHERE m.thread_id = ?
            AND m.edit_of_ts IS NULL
            AND (m.group_call_era IS NULL OR NOT EXISTS (
                     SELECT 1 FROM messages g
                      WHERE g.thread_id = m.thread_id AND g.group_call_era = m.group_call_era
                        AND (g.server_ts < m.server_ts OR (g.server_ts = m.server_ts AND g.id < m.id))))
            AND (? IS NULL OR m.server_ts > ? OR (m.server_ts = ? AND m.id > ?))
          ORDER BY m.server_ts ASC, m.id ASC
          LIMIT ?"
        }
        PageDir::AtAndNewer => {
            r"SELECT m.id AS id, m.server_ts AS ts,
                 COALESCE(ct.display_name, m.sender_uuid) AS sender, m.sender_uuid AS sender_key,
                 m.is_outgoing AS is_outgoing, m.body AS body,
                 m.deleted AS deleted, m.edited AS edited,
                 m.quote_target_ts AS quote_target_ts, m.quote_text AS quote_text,
                 COALESCE(qa.display_name, m.quote_author_uuid) AS quote_author,
                 m.group_call_era AS group_call_era
          FROM messages m
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          LEFT JOIN contacts qa ON qa.uuid = m.quote_author_uuid
          WHERE m.thread_id = ?
            AND m.edit_of_ts IS NULL
            AND (m.group_call_era IS NULL OR NOT EXISTS (
                     SELECT 1 FROM messages g
                      WHERE g.thread_id = m.thread_id AND g.group_call_era = m.group_call_era
                        AND (g.server_ts < m.server_ts OR (g.server_ts = m.server_ts AND g.id < m.id))))
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
    let mut keys = Vec::with_capacity(rows.len());
    let mut quotes: Vec<Quote> = Vec::new();
    for r in rows {
        let id: i64 = r.try_get("id")?;
        let ts: i64 = r.try_get("ts")?;
        if let Some(target_ts) = r.try_get("quote_target_ts")? {
            quotes.push(Quote {
                msg_id: id,
                target_ts,
                author: r.try_get("quote_author")?,
                text: r.try_get("quote_text")?,
            });
        }
        keys.push((ts, id));
        let body: Option<String> = r.try_get("body")?;
        let call = r.try_get::<Option<String>, _>("group_call_era")?.is_some() && body.is_none();
        msgs.push(Message {
            deleted: r.try_get("deleted")?,
            edited: r.try_get("edited")?,
            kind: if call {
                MessageKind::Action
            } else {
                MessageKind::Message
            },
            ..Message::bare(
                id,
                ts,
                r.try_get("sender")?,
                r.try_get("sender_key")?,
                r.try_get("is_outgoing")?,
                if call {
                    Some(GROUP_CALL.to_owned())
                } else {
                    body
                },
            )
        });
    }

    attach_attachments(pool, &mut msgs).await?;
    attach_reactions(pool, thread_id, &mut msgs).await?;
    attach_replies(pool, thread_id, &mut msgs, &quotes).await?;
    attach_read(pool, &mut msgs).await?;

    Ok(Fetched::new(msgs, keys, dir))
}

async fn attach_attachments(pool: &MySqlPool, msgs: &mut [Message]) -> Result<()> {
    let ids: Vec<i64> = msgs.iter().map(|m| m.id).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let sql = in_list(
        "SELECT id, message_id, content_type, file_name, size_bytes, stored_path
           FROM attachments WHERE message_id IN ({in}) ORDER BY id",
        ids.len(),
    );
    let rows = bind_all(sqlx::query(AssertSqlSafe(sql)), &ids)
        .fetch_all(pool)
        .await?;
    let mut by = by_row(msgs);
    for r in rows {
        let Some(m) = by.get_mut(&r.try_get::<i64, _>("message_id")?) else {
            continue;
        };
        let stored_path: Option<String> = r.try_get("stored_path")?;
        m.attachments.push(Attachment {
            id: r.try_get("id")?,
            content_type: r.try_get("content_type")?,
            file_name: r.try_get("file_name")?,
            size: r.try_get("size_bytes")?,
            available: stored_path.is_some(),
            fetch: None,
            // Filled by `attach_transcripts`.
            transcript: None,
        });
    }
    Ok(())
}

/// Reactions key on (thread_id, target_ts = the message's server_ts). Signal
/// keeps one per person per message: each event replaces the last, and a
/// removal leaves none, so only each person's latest event counts.
async fn attach_reactions(pool: &MySqlPool, thread_id: &str, msgs: &mut [Message]) -> Result<()> {
    let ts: Vec<i64> = msgs.iter().map(|m| m.ts).collect();
    if ts.is_empty() {
        return Ok(());
    }
    // Names gathered here rather than by `GROUP_CONCAT`, which truncates at
    // `group_concat_max_len` without an error.
    let sql = in_list(
        "SELECT r.target_ts, r.emoji, COALESCE(ct.display_name, r.author_uuid) AS who
           FROM reactions r
           JOIN (SELECT target_ts, author_uuid, MAX(reaction_ts) AS latest
                   FROM reactions
                  WHERE thread_id = ? AND target_ts IN ({in})
                  GROUP BY target_ts, author_uuid) l
             ON l.target_ts = r.target_ts AND l.author_uuid = r.author_uuid
            AND l.latest = r.reaction_ts
           LEFT JOIN contacts ct ON ct.uuid = r.author_uuid
          WHERE r.thread_id = ? AND r.removed = 0 AND r.emoji IS NOT NULL
          ORDER BY r.target_ts, r.emoji, who",
        ts.len(),
    );
    let rows = bind_all(sqlx::query(AssertSqlSafe(sql)).bind(thread_id), &ts)
        .bind(thread_id)
        .fetch_all(pool)
        .await?;
    let mut by_ts: HashMap<i64, &mut Message> = HashMap::new();
    for m in msgs.iter_mut() {
        by_ts.entry(m.ts).or_insert(m);
    }
    // Sorted, so one emoji's names arrive together, and a name twice in a row.
    for r in rows {
        let Some(m) = by_ts.get_mut(&r.try_get::<i64, _>("target_ts")?) else {
            continue;
        };
        let emoji: String = r.try_get("emoji")?;
        let who: String = r.try_get("who")?;
        match m.reactions.last_mut() {
            // One author reacting twice with an emoji counts and is named once.
            Some(last) if last.emoji == emoji => {
                if last.who.last() != Some(&who) {
                    last.who.push(who);
                    last.count += 1;
                }
            }
            _ => m.reactions.push(Reaction {
                emoji,
                count: 1,
                who: vec![who],
            }),
        }
    }
    Ok(())
}

/// A Signal quote: the target's timestamp, and its author and text as the quote
/// carries them.
struct Quote {
    msg_id: i64,
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
    let sql = in_list(
        "SELECT m.id AS id, m.server_ts AS ts, m.body AS body, m.deleted AS deleted,
                COALESCE(ct.display_name, m.sender_uuid) AS sender
         FROM messages m
         LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
         WHERE m.thread_id = ? AND m.edit_of_ts IS NULL
           AND m.server_ts IN ({in})",
        targets.len(),
    );
    let rows = bind_all(sqlx::query(AssertSqlSafe(sql)).bind(thread_id), &targets)
        .fetch_all(pool)
        .await?;
    let mut bodies: HashMap<i64, String> = HashMap::new();
    for r in &rows {
        // A deleted target's words are withheld.
        if let (false, Some(body)) = (r.try_get("deleted")?, r.try_get("body")?) {
            bodies.insert(r.try_get("id")?, body);
        }
    }
    with_names(pool, &mut bodies).await?;
    let mut found: HashMap<i64, ReplyTo> = HashMap::new();
    for r in &rows {
        let id: i64 = r.try_get("id")?;
        let ts: i64 = r.try_get("ts")?;
        if found.contains_key(&ts) {
            continue;
        }
        found.insert(
            ts,
            ReplyTo {
                id: Some(id),
                cursor: Some(encode_cursor(ts, id)),
                ts: Some(ts),
                sender: Some(r.try_get("sender")?),
                excerpt: excerpt(bodies.get(&id).map(String::as_str)),
                deleted: r.try_get("deleted")?,
            },
        );
    }
    let mut by = by_row(msgs);
    for q in quotes {
        let Some(m) = by.get_mut(&q.msg_id) else {
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
) -> Result<HashMap<i64, i64>> {
    let mut shown = HashMap::new();
    let originals: Vec<i64> = msgs.iter().filter(|m| m.edited).map(|m| m.ts).collect();
    if originals.is_empty() {
        return Ok(shown);
    }
    // Scoped to the thread: `edit_of_ts` is a timestamp, and two threads can
    // share one.
    let sql = in_list(
        "SELECT id, edit_of_ts, server_ts, body FROM messages
         WHERE thread_id = ? AND edit_of_ts IN ({in})
         ORDER BY server_ts ASC",
        originals.len(),
    );
    // Each original's revisions, oldest first: (row, sent, text).
    let mut revisions: HashMap<i64, Vec<(i64, i64, Option<String>)>> = HashMap::new();
    for r in bind_all(sqlx::query(AssertSqlSafe(sql)).bind(thread_id), &originals)
        .fetch_all(pool)
        .await?
    {
        revisions
            .entry(r.try_get("edit_of_ts")?)
            .or_default()
            .push((
                r.try_get("id")?,
                r.try_get("server_ts")?,
                r.try_get("body")?,
            ));
    }
    for m in msgs.iter_mut().filter(|m| m.edited) {
        let Some(mut list) = revisions.remove(&m.ts) else {
            continue;
        };
        let Some((row, _, newest)) = list.pop() else {
            continue;
        };
        // Earlier versions, in order: the original's text, then every revision
        // but the newest, each stamped with when it was sent.
        let stamps: Vec<i64> = std::iter::once(m.ts)
            .chain(list.iter().map(|(_, ts, _)| *ts))
            .collect();
        let texts = std::iter::once(m.body.take()).chain(list.into_iter().map(|(_, _, b)| b));
        m.edits = stamps
            .into_iter()
            .zip(texts)
            .map(|(ts, body)| MessageEdit { ts, body })
            .collect();
        m.body = newest;
        shown.insert(m.id, row);
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
    shown: &HashMap<i64, i64>,
) -> Result<()> {
    let index = shown_rows(msgs, shown);
    if index.is_empty() {
        return Ok(());
    }
    let rows: Vec<i64> = index.keys().copied().collect();
    let sql = in_list(
        "SELECT message_id, style, start_utf16, length_utf16 FROM signal_text_styles
          WHERE message_id IN ({in})
          ORDER BY message_id, start_utf16, length_utf16, style",
        rows.len(),
    );
    for r in bind_all(sqlx::query(AssertSqlSafe(sql)), &rows)
        .fetch_all(pool)
        .await?
    {
        let Some(&i) = index.get(&r.try_get::<i64, _>("message_id")?) else {
            continue;
        };
        let style: String = r.try_get("style")?;
        msgs[i].entities.push(Entity {
            // An unknown style passes through, and renders as plain text.
            kind: match SignalStyle::parse(&style) {
                Some(known) => known.kind().to_owned(),
                None => style,
            },
            offset: r.try_get::<i32, _>("start_utf16")?.into(),
            length: r.try_get::<i32, _>("length_utf16")?.into(),
            url: None,
        });
    }
    Ok(())
}

/// The row whose text each message on the page shows, to its index: its own,
/// or for an edited one its newest revision (`shown`).
fn shown_rows(msgs: &[Message], shown: &HashMap<i64, i64>) -> HashMap<i64, usize> {
    msgs.iter()
        .enumerate()
        .map(|(i, m)| (shown.get(&m.id).copied().unwrap_or(m.id), i))
        .collect()
}

/// One mention, as the viewer names it: where its placeholder is (UTF-16) and
/// the name a sender would get.
struct Named {
    start: i64,
    length: i64,
    who: String,
}

/// The mentions in each of these rows' bodies, last first, so naming one
/// leaves the offsets of those before it alone.
async fn mentions_in(pool: &MySqlPool, rows: &[i64]) -> Result<HashMap<i64, Vec<Named>>> {
    let mut out: HashMap<i64, Vec<Named>> = HashMap::new();
    if rows.is_empty() {
        return Ok(out);
    }
    let sql = in_list(
        "SELECT x.message_id, x.start_utf16, x.length_utf16,
                COALESCE(ct.display_name, x.uuid) AS who
           FROM signal_mentions x
           LEFT JOIN contacts ct ON ct.uuid = x.uuid
          WHERE x.message_id IN ({in})
          ORDER BY x.message_id, x.start_utf16 DESC",
        rows.len(),
    );
    let q = bind_all(sqlx::query(AssertSqlSafe(sql)), rows);
    for r in q.fetch_all(pool).await? {
        let start: i32 = r.try_get("start_utf16")?;
        let length: i32 = r.try_get("length_utf16")?;
        out.entry(r.try_get("message_id")?)
            .or_default()
            .push(Named {
                start: start.into(),
                length: length.into(),
                who: r.try_get("who")?,
            });
    }
    Ok(out)
}

/// Name the people this page's Signal messages mention. Signal sends a mention
/// as U+FFFC in the text; each becomes `@` and the name a sender would get, as
/// a `mention` run, and the runs after it move with the text. Run after
/// `attach_styles`, whose offsets it moves.
pub(super) async fn attach_mentions(
    pool: &MySqlPool,
    msgs: &mut [Message],
    shown: &HashMap<i64, i64>,
) -> Result<()> {
    let index = shown_rows(msgs, shown);
    let rows: Vec<i64> = index.keys().copied().collect();
    let mentions = mentions_in(pool, &rows).await?;
    for (row, i) in index {
        let m = &mut msgs[i];
        for x in mentions.get(&row).into_iter().flatten() {
            let Some(named) = m.body.as_mut().and_then(|b| put_name(b, x)) else {
                continue;
            };
            let (end, moved) = (x.start + x.length, named - x.length);
            for e in &mut m.entities {
                if e.offset >= end {
                    e.offset += moved;
                } else if e.offset + e.length >= end && e.offset <= x.start {
                    e.length += moved;
                }
            }
            m.entities.push(Entity {
                kind: "mention".to_string(),
                offset: x.start,
                length: named,
                url: None,
            });
        }
    }
    Ok(())
}

/// Each body with its mentions named, for text shown without runs: a quote's
/// excerpt, a search hit. Keyed by message row.
async fn with_names(pool: &MySqlPool, bodies: &mut HashMap<i64, String>) -> Result<()> {
    let rows: Vec<i64> = bodies.keys().copied().collect();
    for (row, mentions) in mentions_in(pool, &rows).await? {
        if let Some(body) = bodies.get_mut(&row) {
            for x in &mentions {
                put_name(body, x);
            }
        }
    }
    Ok(())
}

/// Replace the placeholder at `x`'s place with `@who`, giving the name's
/// length in UTF-16. Anything else there is left alone: the text has moved,
/// and a name in the wrong place is worse than a placeholder.
fn put_name(body: &mut String, x: &Named) -> Option<i64> {
    let a = byte_at(body, x.start)?;
    let b = byte_at(body, x.start + x.length)?;
    if a == b || body[a..b].chars().any(|c| c != '\u{FFFC}') {
        return None;
    }
    let name = format!("@{}", x.who);
    body.replace_range(a..b, &name);
    Some(name.encode_utf16().count() as i64)
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
    let ids: Vec<i64> = msgs.iter().map(|m| m.id).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let sql = in_list(
        "SELECT id, message_id, url, title, description, image_path
           FROM signal_link_previews
          WHERE message_id IN ({in})
          ORDER BY message_id, position",
        ids.len(),
    );
    let rows = bind_all(sqlx::query(AssertSqlSafe(sql)), &ids)
        .fetch_all(pool)
        .await?;
    let mut by = by_row(msgs);
    for r in rows {
        let Some(m) = by.get_mut(&r.try_get::<i64, _>("message_id")?) else {
            continue;
        };
        let id: i64 = r.try_get("id")?;
        let held: Option<String> = r.try_get("image_path")?;
        m.previews.push(LinkPreview {
            url: r.try_get("url")?,
            title: r.try_get("title")?,
            description: r.try_get("description")?,
            image: held.map(|_| id.to_string()),
        });
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
        .map(|m| m.id)
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let floor_ms: Option<i64> =
        sqlx::query_scalar("SELECT UNIX_TIMESTAMP(MIN(observed_at)) * 1000 FROM signal_receipts")
            .fetch_optional(pool)
            .await?
            .flatten();
    let sql = in_list(
        "SELECT m.id AS id, r.kind AS kind, r.when_ts AS when_ts,
                COALESCE(ct.display_name, r.author_uuid) AS who
         FROM signal_receipts r
         JOIN messages m ON m.server_ts = r.target_ts
         LEFT JOIN contacts ct ON ct.uuid = r.author_uuid
         WHERE m.id IN ({in}) AND r.author_uuid <> m.sender_uuid
         ORDER BY r.when_ts",
        ids.len(),
    );
    let rows = bind_all(sqlx::query(AssertSqlSafe(sql)), &ids)
        .fetch_all(pool)
        .await?;
    let mut by = by_row(msgs);
    for row in rows {
        let Some(m) = by.get_mut(&row.try_get::<i64, _>("id")?) else {
            continue;
        };
        let kind: String = row.try_get("kind")?;
        let state = match kind.as_str() {
            "delivery" => DeliveryState::Delivered,
            "read" => DeliveryState::Read,
            "viewed" => DeliveryState::Viewed,
            // An unknown kind is a schema change this build has not seen.
            other => bail!("signal_receipts.kind holds an unknown kind: {other:?}"),
        };
        let d = m.delivery.get_or_insert_with(|| Delivery {
            state: DeliveryState::Sent,
            read_by: Vec::new(),
        });
        d.state = d.state.max(state);
        if state >= DeliveryState::Read {
            // Ordered by `when_ts`; a person who read and then viewed is named once.
            let who: String = row.try_get("who")?;
            if !d.read_by.iter().any(|r| r.who == who) {
                d.read_by.push(ReadBy {
                    who,
                    at: row.try_get("when_ts")?,
                });
            }
        }
    }
    drop(by);
    // Nothing came back: meaningful only if we were listening then.
    for m in msgs
        .iter_mut()
        .filter(|m| m.is_outgoing && m.delivery.is_none())
    {
        if floor_ms.is_some_and(|f| m.ts >= f) {
            m.delivery = Some(Delivery {
                state: DeliveryState::Sent,
                read_by: Vec::new(),
            });
        }
    }
    Ok(())
}

/// Stored location and content type of a Signal attachment's bytes, if held.
pub async fn attachment_blob(
    pool: &MySqlPool,
    id: i64,
) -> Result<Option<(Option<String>, String)>> {
    Ok(sqlx::query_as(
        "SELECT content_type, stored_path FROM attachments WHERE id = ? AND stored_path IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// A link preview's stored picture: its content type and where it was stored.
pub async fn preview_image_blob(
    pool: &MySqlPool,
    id: i64,
) -> Result<Option<(Option<String>, String)>> {
    Ok(sqlx::query_as(
        "SELECT image_content_type, image_path FROM signal_link_previews
          WHERE id = ? AND image_path IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
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
        r"SELECT COALESCE(o.id, m.id) AS id, m.id AS row_id, m.thread_id AS cid, c.name AS cname,
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
        r"SELECT COALESCE(o.id, m.id) AS id, m.id AS row_id, m.thread_id AS cid, c.name AS cname,
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
    // The row each hit's text came from: mentions are kept per row.
    let mut matched = Vec::new();
    for r in rows {
        let id: i64 = r.try_get("id")?;
        if !seen.insert(id) {
            continue;
        }
        let ts: i64 = r.try_get("ts")?;
        matched.push(r.try_get::<i64, _>("row_id")?);
        hits.push(SearchHit {
            origin: Origin::Signal,
            conversation_id: r.try_get("cid")?,
            conversation_name: r.try_get("cname")?,
            ts,
            sender: r.try_get("sender")?,
            snippet: r.try_get::<Option<String>, _>("body")?.unwrap_or_default(),
            deleted: r.try_get("deleted")?,
            // Signal's native unit is milliseconds, the same as `ts`.
            cursor: encode_cursor(ts, id),
        });
    }
    let mut bodies: HashMap<i64, String> = matched
        .iter()
        .zip(&hits)
        .map(|(row, h)| (*row, h.snippet.clone()))
        .collect();
    with_names(pool, &mut bodies).await?;
    for (row, h) in matched.iter().zip(&mut hits) {
        if let Some(named) = bodies.remove(row) {
            h.snippet = named;
        }
    }
    Ok(hits)
}
