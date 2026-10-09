//! IRC: irssi's logs, imported and tailed into `irc_*`; the one origin this app
//! can also send to.

use anyhow::{Result, bail};
use sqlx::{MySqlPool, Row};

use super::*;

/// Every IRC conversation but irssi's server-notice window.
pub(super) async fn conversations(pool: &MySqlPool) -> Result<Vec<Conversation>> {
    let mut out = Vec::new();
    // IRC reads `irc_conversation_stats`, which the archiver's triggers maintain.
    // Aggregating `irc_messages` here is too slow for the landing page: the
    // `kind` filter defeats the loose index scan.
    //
    // `TIMESTAMPDIFF` from the epoch, not `UNIX_TIMESTAMP`, which would apply the
    // connection's time zone to the DATETIME.
    //
    // `is_status = 0` drops irssi's server-notice window. A conversation with no
    // counted line has no stats row.
    //
    // The last line is the newest row in the last second: many share it.
    let irc = sqlx::query(
        r"SELECT c.id AS id, c.target AS name, c.is_channel AS is_channel,
                 c.network AS network, COALESCE(s.cnt, 0) AS cnt,
                 TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', s.last_sent_at) AS last_s,
                 l.nick AS last_sender, l.is_self AS last_out, l.text AS last_text,
                 CAST(NULL AS CHAR) AS last_media
          FROM irc_conversations c
          LEFT JOIN irc_conversation_stats s ON s.conversation_id = c.id
          LEFT JOIN irc_messages l
                 ON l.conversation_id = c.id AND l.sent_at = s.last_sent_at
                AND l.kind IN ('message', 'action')
          WHERE c.is_status = 0
          ORDER BY l.id DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut seen = std::collections::HashSet::new();
    for r in irc {
        let id: i32 = r.try_get("id")?;
        if !seen.insert(id) {
            continue;
        }
        let is_channel: i8 = r.try_get("is_channel")?;
        let last_s: Option<i64> = r.try_get("last_s")?;
        out.push(Conversation {
            origin: Origin::Irc,
            id: id.to_string(),
            name: r.try_get("name")?,
            kind: kind_from_is_dm(is_channel == 0),
            network: r.try_get("network")?,
            message_count: r.try_get("cnt")?,
            last_ts: last_s.map(s_to_ms),
            last: LastMessage::from_row(&r, false)?,
            // No read state to read.
            unread: 0,
            // Filled by `avatars::attach`, which knows where the files are.
            avatar: None,
        });
    }
    Ok(out)
}

/// One page of an IRC conversation. irssi logs only `%H:%M`, so many lines share
/// a timestamp and `id` carries the order: the importer writes files in path
/// order, and logs only append. Only messages and actions, the population
/// [`list_conversations`] counts.
pub(super) async fn page(
    pool: &MySqlPool,
    conversation_id: &str,
    cursor: Option<(i64, i64)>,
    limit: i64,
    dir: PageDir,
) -> Result<Fetched> {
    let (cur_ts, cur_id) = (cursor.map(|(ts, _)| ts), cursor.map(|(_, id)| id));
    let sql = match dir {
        PageDir::Older => {
            r"SELECT m.id AS id,
                 TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', m.sent_at) AS ts_s,
                 m.nick AS sender, m.is_self AS is_self, m.text AS body, m.kind AS kind
          FROM irc_messages m
          WHERE m.conversation_id = ?
            AND m.kind IN ('message', 'action')
            AND (? IS NULL
                 OR TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', m.sent_at) < ?
                 OR (TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', m.sent_at) = ?
                     AND m.id < ?))
          ORDER BY ts_s DESC, m.id DESC
          LIMIT ?"
        }
        PageDir::Newer => {
            r"SELECT m.id AS id,
                 TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', m.sent_at) AS ts_s,
                 m.nick AS sender, m.is_self AS is_self, m.text AS body, m.kind AS kind
          FROM irc_messages m
          WHERE m.conversation_id = ?
            AND m.kind IN ('message', 'action')
            AND (? IS NULL
                 OR TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', m.sent_at) > ?
                 OR (TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', m.sent_at) = ?
                     AND m.id > ?))
          ORDER BY ts_s ASC, m.id ASC
          LIMIT ?"
        }
        PageDir::AtAndNewer => {
            r"SELECT m.id AS id,
                 TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', m.sent_at) AS ts_s,
                 m.nick AS sender, m.is_self AS is_self, m.text AS body, m.kind AS kind
          FROM irc_messages m
          WHERE m.conversation_id = ?
            AND m.kind IN ('message', 'action')
            AND (? IS NULL
                 OR TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', m.sent_at) > ?
                 OR (TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', m.sent_at) = ?
                     AND m.id >= ?))
          ORDER BY ts_s ASC, m.id ASC
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
    for r in rows {
        let id: i64 = r.try_get("id")?;
        let ts_s: i64 = r.try_get("ts_s")?;
        keys.push((ts_s, id));
        let is_self: i8 = r.try_get("is_self")?;
        let kind: String = r.try_get("kind")?;
        // The query admits only kinds this parses; anything else is reported
        // rather than drawn as speech.
        let Some(kind) = MessageKind::parse(&kind) else {
            bail!("irc_messages.kind holds a value this query should have excluded: {kind:?}");
        };
        msgs.push(Message {
            kind,
            ..Message::bare(
                id,
                s_to_ms(ts_s),
                r.try_get::<Option<String>, _>("sender")?
                    .unwrap_or_default(),
                is_self != 0,
                r.try_get("body")?,
            )
        });
    }

    Ok(Fetched::new(msgs, keys, dir))
}

/// An IRC conversation's network and target, for sending. Taken from the
/// database, so a request can name only an existing conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrcTarget {
    /// The conversation's network, after any `--map` the importer applied.
    pub network: String,
    /// A nick, or a channel including its leading `#`.
    pub target: String,
    /// irssi's server-notice window, named after your own nick. Sending to it
    /// would message yourself as though you were somebody else.
    pub is_status: bool,
}

/// Look up an IRC conversation's network and target. `None` if there is no such
/// conversation.
pub async fn target(pool: &MySqlPool, conversation_id: &str) -> Result<Option<IrcTarget>> {
    let row = sqlx::query("SELECT network, target, is_status FROM irc_conversations WHERE id = ?")
        .bind(conversation_id)
        .fetch_optional(pool)
        .await?;
    let Some(r) = row else { return Ok(None) };
    let is_status: i8 = r.try_get("is_status")?;
    Ok(Some(IrcTarget {
        network: r.try_get("network")?,
        target: r.try_get("target")?,
        is_status: is_status != 0,
    }))
}

/// IRC speech matching `like`, in one conversation when `id` is given.
pub(super) async fn search(
    pool: &MySqlPool,
    like: &str,
    id: Option<&str>,
    limit: i64,
) -> Result<Vec<SearchHit>> {
    let mut hits = Vec::new();
    // Messages and actions only. `LIKE '%term%'` cannot use an index, so
    // the global form scans every line; the scoped form reads one conversation
    // through the `conversation_id` index. Cost follows match density: `LIMIT`
    // stops the scan early when matches are common.
    //
    // The scan runs alone in a derived table and is joined afterwards. Joined
    // directly, the optimizer drives from `irc_conversations` into random reads
    // of `irc_messages`. `is_status` is a subquery inside, so it filters before
    // `LIMIT` rather than shortening the page after it.
    let sql = if id.is_some() {
        r"SELECT m.conversation_id AS cid, c.target AS cname,
                 TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', m.sent_at) AS ts_s,
                 m.nick AS sender, m.text AS body, m.id AS id
          FROM (
              SELECT conversation_id, sent_at, id, nick, text
                FROM irc_messages
               WHERE kind IN ('message', 'action')
                 AND conversation_id = ?
                 AND text LIKE ?
                 AND conversation_id NOT IN (
                     SELECT id FROM irc_conversations WHERE is_status = 1
                 )
               ORDER BY sent_at DESC, id DESC LIMIT ?
          ) m
          LEFT JOIN irc_conversations c ON c.id = m.conversation_id
          ORDER BY m.sent_at DESC, m.id DESC"
    } else {
        r"SELECT m.conversation_id AS cid, c.target AS cname,
                 TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', m.sent_at) AS ts_s,
                 m.nick AS sender, m.text AS body, m.id AS id
          FROM (
              SELECT conversation_id, sent_at, id, nick, text
                FROM irc_messages
               WHERE kind IN ('message', 'action')
                 AND text LIKE ?
                 AND conversation_id NOT IN (
                     SELECT id FROM irc_conversations WHERE is_status = 1
                 )
               ORDER BY sent_at DESC, id DESC LIMIT ?
          ) m
          LEFT JOIN irc_conversations c ON c.id = m.conversation_id
          ORDER BY m.sent_at DESC, m.id DESC"
    };
    // The scoped literal binds the id first, in the order of its `?`.
    let mut q = sqlx::query(sql);
    if let Some(id) = id {
        q = q.bind(id);
    }
    let rows = q.bind(like).bind(limit).fetch_all(pool).await?;
    for r in rows {
        let cid: i32 = r.try_get("cid")?;
        let ts_s: i64 = r.try_get("ts_s")?;
        let id: i64 = r.try_get("id")?;
        hits.push(SearchHit {
            origin: Origin::Irc,
            conversation_id: cid.to_string(),
            conversation_name: r.try_get("cname")?,
            ts: s_to_ms(ts_s),
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            snippet: r.try_get::<Option<String>, _>("body")?.unwrap_or_default(),
            deleted: false,
            // Whole seconds; `id` orders lines within a minute.
            cursor: encode_cursor(ts_s, id),
        });
    }
    Ok(hits)
}
