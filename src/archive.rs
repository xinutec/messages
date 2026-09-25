//! Read-only queries over the message archive, normalising Signal, Google Chat,
//! IRC and Telegram into one shape for the UI. The only write, the echo of a sent
//! IRC line, is in [`crate::irc_send`].

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::collections::HashMap;

use crate::link_image::{LinkState, askable};
use sqlx::{AssertSqlSafe, MySqlPool, Row};

/// Which archive a conversation came from: the URL segment, the `origin` field
/// and every per-origin match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub enum Origin {
    Signal,
    Gchat,
    Irc,
    Telegram,
}

impl Origin {
    /// Parse the `{origin}` URL segment; `None` becomes a 404.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "signal" => Some(Origin::Signal),
            "gchat" => Some(Origin::Gchat),
            "irc" => Some(Origin::Irc),
            "telegram" => Some(Origin::Telegram),
            _ => None,
        }
    }
}

/// Whether a conversation is one-to-one, a group, or a broadcast. `channel` is
/// Telegram's alone, kept separate so a reader can leave broadcasts out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub enum ConversationKind {
    Dm,
    Group,
    /// A broadcast with an audience; Telegram only.
    Channel,
}

impl ConversationKind {
    /// Parse a conversation-kind ENUM value. `None` means the schema has a kind
    /// this build does not know, which the caller reports.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "dm" => Some(ConversationKind::Dm),
            "group" => Some(ConversationKind::Group),
            "channel" => Some(ConversationKind::Channel),
            _ => None,
        }
    }
}

// ts-rs copies doc comments into `generated/`, so they carry no intra-doc links.
/// Whether a line was said or done.
///
/// Two of `irc_messages.kind`'s four values: joins, parts and notices are not
/// conversation. Telegram service events are `action`; Signal and Google Chat are
/// always `message`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub enum MessageKind {
    /// Someone said something.
    Message,
    /// Someone did something, phrased in the third person about the sender.
    Action,
}

impl MessageKind {
    /// Parse an `irc_messages.kind` value; `None` for a kind the queries exclude.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "message" => Some(MessageKind::Message),
            "action" => Some(MessageKind::Action),
            _ => None,
        }
    }
}

/// Telegram and IRC store seconds; the API uses milliseconds.
pub fn s_to_ms(s: i64) -> i64 {
    s * 1_000
}

/// Google Chat stores microsecond timestamps; the unified API uses milliseconds.
pub fn us_to_ms(us: i64) -> i64 {
    us / 1000
}

/// A cursor landing on the first message of a day, in the origin's native unit.
///
/// The id is 0, a floor below every real row, so the paging predicate
/// `ts > ? OR (ts = ? AND id >= ?)` admits everything in the first second.
///
/// `day_start_ms` is the day's start in epoch milliseconds.
pub fn cursor_for_day(origin: Origin, day_start_ms: i64) -> String {
    let native = match origin {
        Origin::Signal => day_start_ms,
        // The only origin that multiplies.
        Origin::Gchat => day_start_ms * 1000,
        Origin::Telegram | Origin::Irc => day_start_ms / 1000,
    };
    encode_cursor(native, 0)
}

/// Google Chat stores only `is_dm`, so its kind is derived.
pub fn kind_from_is_dm(is_dm: bool) -> ConversationKind {
    if is_dm {
        ConversationKind::Dm
    } else {
        ConversationKind::Group
    }
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

/// Opaque pagination cursor: the native `(ts, id)` of a page's end row. The id
/// breaks ties between rows sharing a timestamp, and the native unit keeps Google
/// Chat's microseconds apart.
pub fn encode_cursor(native_ts: i64, id: i64) -> String {
    format!("{native_ts}_{id}")
}

/// Parse a cursor minted by [`encode_cursor`]; `None` for anything malformed.
pub fn parse_cursor(s: &str) -> Option<(i64, i64)> {
    let (ts, id) = s.split_once('_')?;
    Some((ts.parse().ok()?, id.parse().ok()?))
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct Conversation {
    pub origin: Origin,
    pub id: String,
    pub name: Option<String>,
    pub kind: ConversationKind,
    /// The IRC network; `None` elsewhere. `name` alone is only the target.
    pub network: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub message_count: i64,
    /// Epoch milliseconds; None for a conversation with no messages.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_ts: Option<i64>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct Reaction {
    pub emoji: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub count: i64,
    /// Who reacted, where the origin records it. Empty means not recorded, and it
    /// can be shorter than `count` (Telegram truncates), so `count` is
    /// authoritative.
    pub who: Vec<String>,
}

/// How far an outgoing message got. Each origin reaches only the states it can
/// report: Telegram has no `Delivered`, only a read position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub enum DeliveryState {
    /// Sent, with nothing reported back yet.
    Sent,
    /// Their device has it. Signal only.
    Delivered,
    /// Read past it.
    Read,
    /// View-once media opened. Signal only; above `Read`, which it implies.
    Viewed,
}

/// One person, and when they read it.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct ReadBy {
    pub who: String,
    /// Epoch milliseconds, as Signal reported the read.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at: i64,
}

/// What happened to a message we sent.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct Delivery {
    pub state: DeliveryState,
    /// Who read it and when, Signal only. Only those who sent a receipt: in a
    /// group it is never the membership. Telegram names nobody, so it is empty.
    pub read_by: Vec<ReadBy>,
}

/// Whether an attachment's bytes can be asked for. `None` means there is nothing
/// to ask: Signal's are fetched on arrival or never.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub enum FetchState {
    /// Telegram has it; nobody has asked. A reader may.
    Offered,
    /// Asked for, and the feed has not delivered it yet.
    Wanted,
    /// Tried and could not. A reader may ask again.
    Failed,
}

impl FetchState {
    /// Parse a `telegram_media.state` value. `stored` is expressed by
    /// `available` instead.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "offered" => Some(FetchState::Offered),
            "wanted" => Some(FetchState::Wanted),
            "failed" => Some(FetchState::Failed),
            _ => None,
        }
    }
}

/// The attachment fields that change while a fetch is in flight, shaped as on
/// `Attachment`.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct MediaState {
    pub available: bool,
    pub fetch: Option<FetchState>,
    pub content_type: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct Attachment {
    pub id: String,
    pub content_type: Option<String>,
    pub file_name: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub size: Option<i64>,
    /// Whether the bytes are held.
    pub available: bool,
    pub is_image: bool,
    /// Whether these bytes can be asked for.
    pub fetch: Option<FetchState>,
}

/// An earlier version of an edited message. The current text is the message's
/// `body`.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct MessageEdit {
    /// When this version was sent. Epoch milliseconds.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ts: i64,
    pub body: Option<String>,
}

/// One run of formatting inside a message body — bold, a link, a spoiler.
///
/// Offsets are UTF-16 code units, Telegram's unit and JavaScript's string
/// index, so they reach the browser unconverted. `url` comes from the sender and
/// is only ever bound through Angular's sanitising `[href]`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct Entity {
    /// Telegram's own name: `bold`, `italic`, `url`, `textUrl`, `strike`,
    /// `code`, `spoiler`, … A string, so an unknown kind renders as plain text.
    pub kind: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub offset: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub length: i64,
    /// Where a `textUrl` points; `None` for `url`, whose text is the link.
    pub url: Option<String>,
}

/// A picture we hold for a link in the message; see `link_image.rs`.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct LinkImage {
    /// The link as it appears in the message.
    pub url: String,
    /// Our handle for the bytes: `/api/link-images/{id}`.
    pub id: String,
    pub content_type: String,
}

#[derive(Serialize, Clone)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
/// What a message replied to. The target may not be in the archive; the reply
/// still renders, with only what is known.
pub struct ReplyTo {
    /// The target's API id, or `None` when the archive does not hold it.
    pub id: Option<String>,
    /// Cursor for `?at`, landing on the target. `None` exactly when `id` is.
    pub cursor: Option<String>,
    /// Epoch milliseconds of the message replied to, when known.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub ts: Option<i64>,
    pub sender: Option<String>,
    /// A short prefix of the target's text; `None` without text or when deleted.
    pub excerpt: Option<String>,
    /// The target is held but deleted, so its excerpt is withheld.
    pub deleted: bool,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct Message {
    pub id: String,
    /// Epoch milliseconds.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ts: i64,
    pub sender: String,
    pub is_outgoing: bool,
    /// Said or done. Always `message` outside IRC.
    pub kind: MessageKind,
    pub body: Option<String>,
    pub deleted: bool,
    pub edited: bool,
    pub reactions: Vec<Reaction>,
    pub attachments: Vec<Attachment>,
    /// Pictures we hold for links in `body`.
    pub link_images: Vec<LinkImage>,
    /// Earlier versions, oldest first; `body` is the current text.
    pub edits: Vec<MessageEdit>,
    /// Links in `body` we could fetch a picture for, if asked.
    pub link_offers: Vec<LinkOffer>,
    /// What this message replied to. Signal, Telegram and Google Chat.
    pub reply_to: Option<ReplyTo>,
    /// How far this message got. `None` means the archive cannot say: incoming
    /// messages, origins without receipts, and anything sent before capture began.
    pub delivery: Option<Delivery>,
    /// Formatting runs in `body`: Telegram's entities, and Signal's text styles
    /// under the same kinds. Empty means none recorded.
    pub entities: Vec<Entity>,
    /// The album this message was sent in, shared by its members: Telegram's
    /// `grouped_id`, as a string because it exceeds a JavaScript number.
    pub album: Option<String>,
    /// Link previews the sender's app attached. Signal only.
    pub previews: Vec<LinkPreview>,
}

/// A link preview as the sender's app made it.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct LinkPreview {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
}

/// A link the reader can ask us to fetch a picture for.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct LinkOffer {
    /// The link as typed.
    pub url: String,
    /// Its handle for `POST /api/link-images/{id}/request`.
    pub id: String,
}

fn is_image(ct: Option<&str>) -> bool {
    ct.is_some_and(|c| c.starts_with("image/"))
}

/// One stored version of a message: when it was sent, and what it said.
type Version = (i64, Option<String>);

/// How much of a quoted message a reply preview carries.
pub const EXCERPT_CHARS: usize = 120;

/// A one-line prefix of a quoted message.
///
/// Truncated by characters, since slicing a `String` by bytes panics
/// mid-codepoint. Newlines are folded into one line.
pub fn excerpt(body: Option<&str>) -> Option<String> {
    let flat = body?.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return None;
    }
    let mut out: String = flat.chars().take(EXCERPT_CHARS).collect();
    if flat.chars().count() > EXCERPT_CHARS {
        out.push('…');
    }
    Some(out)
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
async fn attach_signal_replies(
    pool: &MySqlPool,
    thread_id: &str,
    msgs: &mut [Message],
    quotes: &[Quote],
) -> Result<()> {
    if quotes.is_empty() {
        return Ok(());
    }
    let targets: Vec<i64> = {
        let mut t: Vec<i64> = quotes.iter().map(|q| q.target_ts).collect();
        t.sort_unstable();
        t.dedup();
        t
    };
    let placeholders = vec!["?"; targets.len()].join(",");
    let sql = format!(
        "SELECT m.id AS id, m.server_ts AS ts, m.body AS body, m.deleted AS deleted,
                COALESCE(ct.display_name, m.sender_uuid) AS sender
         FROM messages m
         LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
         WHERE m.thread_id = ? AND m.edit_of_ts IS NULL
           AND m.server_ts IN ({placeholders})",
    );
    // A fixed template with a computed count of `?` and every value bound.
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
        m.reply_to = Some(found.get(&q.target_ts).cloned().unwrap_or(ReplyTo {
            id: None,
            cursor: None,
            ts: Some(q.target_ts),
            // Not held: who and what, as the quote itself carries them.
            sender: q.author.clone(),
            excerpt: excerpt(q.text.as_deref()),
            deleted: false,
        }));
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
async fn attach_gchat_attachments(
    pool: &MySqlPool,
    msgs: &mut [Message],
    ids: &[i64],
) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT id, message_id, name, mime, stored_path
           FROM gchat_attachments WHERE message_id IN ({placeholders})
          ORDER BY id",
    );
    // A fixed template with a computed count of `?` and every value bound.
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

/// Resolve Google Chat's quote-replies for one page. They name the target's message id within the group.
/// Not `thread_id`, which is the topic: a DM message is its own topic but can
/// still quote another.
async fn attach_gchat_replies(
    pool: &MySqlPool,
    group_id: &str,
    msgs: &mut [Message],
    quoted: &[(String, String)],
) -> Result<()> {
    if quoted.is_empty() {
        return Ok(());
    }
    let targets: Vec<String> = {
        let mut t: Vec<String> = quoted.iter().map(|(_, q)| q.clone()).collect();
        t.sort();
        t.dedup();
        t
    };
    let placeholders = vec!["?"; targets.len()].join(",");
    let sql = format!(
        "SELECT m.id AS id, m.msg_id AS msg_id, m.ts_us AS ts_us,
                m.sender_name AS sender, m.text AS body
           FROM gchat_messages m
          WHERE m.group_id = ? AND m.msg_id IN ({placeholders})",
    );
    // A fixed template with a computed count of `?` and every value bound.
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
        m.reply_to = Some(found.get(target).cloned().unwrap_or(ReplyTo {
            id: None,
            cursor: None,
            ts: None,
            sender: None,
            excerpt: None,
            deleted: false,
        }));
    }
    Ok(())
}

/// Resolve Telegram's replies for one page. Service events resolve too, since
/// the page returns them: an id in a [`ReplyTo`] must be somewhere the reader can
/// be taken.
async fn attach_telegram_replies(
    pool: &MySqlPool,
    conversation_id: i64,
    msgs: &mut [Message],
    replies: &[(String, i32)],
) -> Result<()> {
    if replies.is_empty() {
        return Ok(());
    }
    let targets: Vec<i32> = {
        let mut t: Vec<i32> = replies.iter().map(|(_, m)| *m).collect();
        t.sort_unstable();
        t.dedup();
        t
    };
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
    // A fixed template with a computed count of `?` and every value bound.
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
        m.reply_to = Some(found.get(target).cloned().unwrap_or(ReplyTo {
            id: None,
            cursor: None,
            // A Telegram reply names an id, which carries no time.
            ts: None,
            sender: None,
            excerpt: None,
            deleted: false,
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
async fn attach_edits(
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
    // A fixed template with a computed count of `?` and every value bound.
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
async fn attach_signal_styles(
    pool: &MySqlPool,
    msgs: &mut [Message],
    shown: &HashMap<String, i64>,
) -> Result<()> {
    let rows: Vec<(usize, i64)> = msgs
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let row = shown.get(&m.id).copied().or_else(|| m.id.parse().ok())?;
            Some((i, row))
        })
        .collect();
    if rows.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; rows.len()].join(",");
    let sql = format!(
        "SELECT message_id, style, start_utf16, length_utf16 FROM signal_text_styles
          WHERE message_id IN ({placeholders})
          ORDER BY message_id, start_utf16, length_utf16, style",
    );
    // A fixed template with a computed count of `?` and every value bound.
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

/// Link previews for this page's Signal messages, in the order sent.
async fn attach_signal_previews(pool: &MySqlPool, msgs: &mut [Message]) -> Result<()> {
    let ids: Vec<i64> = msgs.iter().filter_map(|m| m.id.parse().ok()).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT message_id, url, title, description FROM signal_link_previews
          WHERE message_id IN ({placeholders})
          ORDER BY message_id, position",
    );
    // A fixed template with a computed count of `?` and every value bound.
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for id in &ids {
        q = q.bind(id);
    }
    for r in q.fetch_all(pool).await? {
        let id: i64 = r.try_get("message_id")?;
        let id = id.to_string();
        if let Some(m) = msgs.iter_mut().find(|m| m.id == id) {
            m.previews.push(LinkPreview {
                url: r.try_get("url")?,
                title: r.try_get("title")?,
                description: r.try_get("description")?,
            });
        }
    }
    Ok(())
}

/// Hang any pictures we hold onto the messages whose text linked them.
///
/// One query for the page, over the links in the bodies already fetched.
pub async fn attach_link_images(pool: &MySqlPool, msgs: &mut [Message]) -> Result<()> {
    let mut wanted: Vec<(String, String, usize)> = Vec::new(); // (hash, url, message index)
    for (i, m) in msgs.iter().enumerate() {
        let Some(body) = m.body.as_deref() else {
            continue;
        };
        for url in crate::link_image::urls_in(body) {
            wanted.push((crate::link_fetch::url_hash(&url), url.to_string(), i));
        }
    }
    if wanted.is_empty() {
        return Ok(());
    }
    let hashes: Vec<&str> = {
        let mut h: Vec<&str> = wanted.iter().map(|(h, _, _)| h.as_str()).collect();
        h.sort_unstable();
        h.dedup();
        h
    };
    let placeholders = vec!["?"; hashes.len()].join(",");
    // Every state: a decided link must not be offered, and a control for one
    // would 404.
    let sql = format!(
        "SELECT url_hash, state, content_type, decided_by FROM link_images
         WHERE url_hash IN ({placeholders})",
    );
    // A fixed template with a computed count of `?` and every value bound.
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for h in &hashes {
        q = q.bind(*h);
    }
    let mut known: Vec<(String, LinkState, Option<String>, Option<i32>)> = Vec::new();
    for row in q.fetch_all(pool).await? {
        let raw: String = row.try_get("state")?;
        let Some(state) = LinkState::parse(&raw) else {
            bail!("link_images.state holds {raw:?}, which is not a state this app writes");
        };
        known.push((
            row.try_get("url_hash")?,
            state,
            row.try_get("content_type")?,
            row.try_get("decided_by")?,
        ));
    }

    // Per link: a picture we hold is rendered; one not yet decided is offered;
    // one decided against stays a link.
    let mut unseen: Vec<(&str, &str)> = Vec::new();
    for (hash, url, i) in &wanted {
        let already_offered = msgs[*i].link_offers.iter().any(|o| o.id == *hash);
        match known.iter().find(|(h, _, _, _)| h == hash) {
            Some((_, LinkState::Ok, Some(content_type), _)) => {
                msgs[*i].link_images.push(LinkImage {
                    url: url.clone(),
                    id: hash.clone(),
                    content_type: content_type.clone(),
                });
            }
            // `link_image::askable` decides; the request endpoint asks it too.
            Some((_, state, _, by)) if askable(*state, *by) => {
                if !already_offered {
                    msgs[*i].link_offers.push(LinkOffer {
                        id: hash.clone(),
                        url: url.clone(),
                    });
                }
            }
            Some(_) => {} // decided by this reader: not a picture
            None => {
                if !unseen.iter().any(|(h, _)| h == hash) {
                    unseen.push((hash.as_str(), url.as_str()));
                }
                if !already_offered {
                    msgs[*i].link_offers.push(LinkOffer {
                        id: hash.clone(),
                        url: url.clone(),
                    });
                }
            }
        }
    }
    offer(pool, &unseen).await?;
    Ok(())
}

/// Register this page's links so they can be asked for later. This is the only
/// place a URL enters `link_images`, and it comes from a message, never a
/// request, so a tap cannot make this an open proxy. `INSERT IGNORE` leaves an
/// existing row, and its decision, untouched.
async fn offer(pool: &MySqlPool, links: &[(&str, &str)]) -> Result<()> {
    if links.is_empty() {
        return Ok(());
    }
    let rows = vec!["(?, ?, 'offered', NOW())"; links.len()].join(",");
    let sql =
        format!("INSERT IGNORE INTO link_images (url_hash, url, state, wanted_at) VALUES {rows}");
    // A fixed template with a computed count of `?` and every value bound.
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for (hash, url) in links {
        q = q.bind(*hash).bind(*url);
    }
    q.execute(pool).await.context("offering links")?;
    Ok(())
}

/// The URL of a link we offered, by its hash, so a request never names an
/// address.
pub async fn offered_url(pool: &MySqlPool, id: &str) -> Result<Option<String>> {
    let row: Option<(String, String, Option<i32>)> =
        sqlx::query_as("SELECT url, state, decided_by FROM link_images WHERE url_hash = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row
        .filter(|(_, state, by)| LinkState::parse(state).is_some_and(|s| askable(s, *by)))
        .map(|(url, _, _)| url))
}

/// A link's state and, once there is a picture, its type.
pub async fn link_image_state(
    pool: &MySqlPool,
    id: &str,
) -> Result<Option<(String, Option<String>)>> {
    Ok(
        sqlx::query_as("SELECT state, content_type FROM link_images WHERE url_hash = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// Where a link's stored bytes are, if we hold them.
pub async fn link_image_blob(pool: &MySqlPool, id: &str) -> Result<Option<(String, String)>> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT content_type, stored_name FROM link_images WHERE url_hash = ? AND state = 'ok'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some((Some(ct), Some(name))) => Some((ct, name)),
        _ => None,
    })
}

/// What the archive now holds for one Telegram message, for a reader polling a
/// fetch it asked for: the fetch happens later, in another process.
pub async fn telegram_media_state(pool: &MySqlPool, row_id: i64) -> Result<Option<MediaState>> {
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
pub async fn request_telegram_media(pool: &MySqlPool, row_id: i64) -> Result<bool> {
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
pub async fn telegram_media_blob(
    pool: &MySqlPool,
    row_id: i64,
) -> Result<Option<(Option<String>, String)>> {
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

/// The bytes held for one Google Chat attachment.
pub async fn gchat_attachment_blob(
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

/// Which way a page runs from its cursor. `ORDER BY` takes no bound parameter,
/// so each origin has one literal query per direction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageDir {
    /// Back in time, strictly before the cursor.
    Older,
    /// Forward in time, strictly after the cursor.
    Newer,
    /// The cursor's own row and forward: landing. `Older` and `Newer` are both
    /// strict, so together they would skip the target.
    AtAndNewer,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct MessagesPage {
    /// Ascending by ts.
    pub messages: Vec<Message>,
    pub has_more: bool,
    /// Cursor for the page's oldest row, to continue backwards (`dir=older`).
    /// Both cursors name the page's own ends, whichever way it was fetched.
    pub next_cursor: Option<String>,
    /// Cursor for the page's newest row, to continue forwards (`dir=newer`).
    pub prev_cursor: Option<String>,
}

/// All conversations across all origins, newest activity first.
pub async fn list_conversations(pool: &MySqlPool) -> Result<Vec<Conversation>> {
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
            origin: Origin::Gchat,
            id: r.try_get("id")?,
            name: r.try_get("name")?,
            kind: kind_from_is_dm(is_dm != 0),
            network: None,
            message_count: r.try_get("cnt")?,
            last_ts: last_us.map(us_to_ms),
        });
    }

    // Telegram counts only `kind = 'message'`. Aggregated in a derived table and
    // then joined, so MariaDB answers from `idx_tg_conv_kind_ts`.
    let telegram = sqlx::query(
        r"SELECT t.id AS id, t.kind AS kind, t.name AS name,
                 COALESCE(s.cnt, 0) AS cnt, s.last_ts AS last_ts
          FROM telegram_conversations t
          LEFT JOIN (
              SELECT conversation_id, COUNT(*) AS cnt, MAX(sent_at) AS last_ts
              FROM telegram_messages
              WHERE kind = 'message'
              GROUP BY conversation_id
          ) s ON s.conversation_id = t.id",
    )
    .fetch_all(pool)
    .await?;
    for r in telegram {
        let kind: String = r.try_get("kind")?;
        let Some(kind) = ConversationKind::parse(&kind) else {
            bail!("telegram_conversations.kind holds an unknown kind: {kind:?}");
        };
        let last_s: Option<i64> = r.try_get("last_ts")?;
        out.push(Conversation {
            origin: Origin::Telegram,
            id: r.try_get::<i64, _>("id")?.to_string(),
            name: r.try_get("name")?,
            kind,
            network: None,
            message_count: r.try_get("cnt")?,
            last_ts: last_s.map(s_to_ms),
        });
    }

    // IRC reads `irc_conversation_stats`, maintained by triggers (the
    // archiver's migrations v11-v14). Aggregating `irc_messages` here instead is too slow
    // to serve the landing page: the `kind` filter defeats the loose index scan.
    //
    // `TIMESTAMPDIFF` from the epoch, not `UNIX_TIMESTAMP`, which would apply the
    // connection's time zone to the DATETIME.
    //
    // `is_status = 0` drops irssi's server-notice window. A conversation with no
    // counted line has no stats row.
    let irc = sqlx::query(
        r"SELECT c.id AS id, c.target AS name, c.is_channel AS is_channel,
                 c.network AS network, COALESCE(s.cnt, 0) AS cnt,
                 TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', s.last_sent_at) AS last_s
          FROM irc_conversations c
          LEFT JOIN irc_conversation_stats s ON s.conversation_id = c.id
          WHERE c.is_status = 0",
    )
    .fetch_all(pool)
    .await?;
    for r in irc {
        let id: i32 = r.try_get("id")?;
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
        });
    }

    out.sort_by_key(|c| std::cmp::Reverse(c.last_ts));
    Ok(out)
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
pub async fn irc_target(pool: &MySqlPool, conversation_id: &str) -> Result<Option<IrcTarget>> {
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

/// Who reacted to each of these messages, by `(msg_id, emoji)`. A reactor is
/// named by their conversation, else by any message they sent (the self user has
/// no conversation row), else by their id as text.
async fn telegram_reactors(
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
    // A fixed template with a computed count of `?` and every value bound.
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

/// A call in words, as a verb phrase about the caller: an `Action` renders as
/// `* nick <body>`. No duration means the call was not answered, not that it
/// took no time. `None` when this is not a call.
pub fn call_text(duration_s: Option<i32>, reason: Option<&str>, video: bool) -> Option<String> {
    // A call row exists only for a call, so either column marks one.
    if duration_s.is_none() && reason.is_none() {
        return None;
    }
    let kind = if video { "video call" } else { "call" };
    Some(match (duration_s, reason) {
        (Some(secs), _) => format!("made a {} {kind}", human_duration(secs)),
        (None, Some("missed")) => format!("made a {kind} that went unanswered"),
        (None, Some("busy")) => format!("made a {kind}, the line was busy"),
        (None, Some("disconnect")) => format!("made a {kind} that dropped"),
        (None, Some(other)) => format!("made a {kind} ({other})"),
        (None, None) => format!("made a {kind}"),
    })
}

/// Seconds as a person would say them, coarsely.
fn human_duration(secs: i32) -> String {
    let secs = secs.max(0);
    match secs {
        0..=59 => format!("{secs}-second"),
        60..=3599 => {
            let mins = (secs + 30) / 60;
            format!("{mins}-minute")
        }
        _ => {
            let hours = secs / 3600;
            let mins = (secs % 3600 + 30) / 60;
            if mins == 0 {
                format!("{hours}-hour")
            } else {
                format!("{hours}h{mins:02}m")
            }
        }
    }
}

/// One page of a conversation, oldest→newest, with reactions attached.
///
/// `cursor` is a previous page's `next_cursor` (older) or `prev_cursor` (newer);
/// `dir` says which. `None` starts at the newest page.
pub async fn messages_page(
    pool: &MySqlPool,
    origin: Origin,
    id: &str,
    cursor: Option<(i64, i64)>,
    limit: i64,
    dir: PageDir,
) -> Result<MessagesPage> {
    let mut page = match origin {
        Origin::Signal => signal_messages(pool, id, cursor, limit, dir).await?,
        Origin::Gchat => gchat_messages(pool, id, cursor, limit, dir).await?,
        Origin::Irc => irc_messages(pool, id, cursor, limit, dir).await?,
        Origin::Telegram => telegram_messages(pool, id, cursor, limit, dir).await?,
    };
    // Signal stores an edit as a new row pointing at the original; Telegram
    // edits in place and files the old text beside it.
    match origin {
        Origin::Signal => {
            let shown = attach_edits(pool, id, &mut page.msgs).await?;
            attach_signal_styles(pool, &mut page.msgs, &shown).await?;
            attach_signal_previews(pool, &mut page.msgs).await?;
        }
        Origin::Telegram => attach_telegram_edits(pool, id, &mut page.msgs).await?,
        // Neither has revisions.
        Origin::Gchat | Origin::Irc => {}
    }
    attach_link_images(pool, &mut page.msgs).await?;
    let has_more = page.msgs.len() as i64 == limit;
    Ok(MessagesPage {
        messages: page.msgs,
        has_more,
        next_cursor: page.oldest.map(|(ts, id)| encode_cursor(ts, id)),
        prev_cursor: page.newest.map(|(ts, id)| encode_cursor(ts, id)),
    })
}

/// A fetcher's page, ascending whatever the direction, and the native `(ts, id)`
/// of its ends.
struct Fetched {
    msgs: Vec<Message>,
    oldest: Option<(i64, i64)>,
    newest: Option<(i64, i64)>,
}

impl Fetched {
    /// Rows arrive in the query's order; `dir` says what that order was.
    fn new(mut msgs: Vec<Message>, mut keys: Vec<(i64, i64)>, dir: PageDir) -> Self {
        if dir == PageDir::Older {
            msgs.reverse();
            keys.reverse();
        }
        Self {
            msgs,
            oldest: keys.first().copied(),
            newest: keys.last().copied(),
        }
    }
}

async fn signal_messages(
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
    // `ORDER BY` takes no bound parameter, hence one literal per direction.
    // dev-lint: allow-sqlx — `sql` is one of the three literals directly above.
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
            id: id.to_string(),
            ts,
            sender: r.try_get("sender")?,
            is_outgoing: is_outgoing != 0,
            kind: MessageKind::Message,
            body: r.try_get("body")?,
            deleted: deleted != 0,
            edited: edited != 0,
            reactions: Vec::new(),
            attachments: Vec::new(),
            edits: Vec::new(),
            link_images: Vec::new(),
            link_offers: Vec::new(),
            reply_to: None,
            delivery: None,
            entities: Vec::new(),
            album: None,
            previews: Vec::new(),
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
        // A fixed template with a computed count of `?` and every value bound.
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

    attach_signal_replies(pool, thread_id, &mut msgs, &quotes).await?;
    attach_signal_read(pool, &mut msgs).await?;

    Ok(Fetched::new(msgs, keys, dir))
}

/// Signal delivery and read state for this page's outgoing messages.
///
/// Receipts from the message's own sender are dropped: reading a thread on a
/// linked device syncs a read of my own messages too.
///
/// A message with no receipt is `Sent` only if it postdates the first receipt
/// ever captured; before that, the archive cannot say.
async fn attach_signal_read(pool: &MySqlPool, msgs: &mut [Message]) -> Result<()> {
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
    // A fixed template with a computed count of `?` and every value bound.
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

async fn gchat_messages(
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
    // `ORDER BY` takes no bound parameter, hence one literal per direction.
    // dev-lint: allow-sqlx — `sql` is one of the three literals directly above.
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
        msgs.push(Message {
            id: id.to_string(),
            ts: us_to_ms(ts_us),
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            is_outgoing: is_self != 0,
            kind: MessageKind::Message,
            body: r.try_get("body")?,
            deleted: false,
            edited: false,
            reactions: Vec::new(),
            attachments: Vec::new(),
            edits: Vec::new(),
            link_images: Vec::new(),
            link_offers: Vec::new(),
            reply_to: None,
            delivery: None,
            entities: Vec::new(),
            album: None,
            previews: Vec::new(),
        });
    }

    attach_gchat_replies(pool, group_id, &mut msgs, &quoted).await?;
    attach_gchat_attachments(pool, &mut msgs, &ids).await?;

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
        // A fixed template with a computed count of `?` and every value bound.
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

/// One page of an IRC conversation. irssi logs only `%H:%M`, so many lines share
/// a timestamp and `id` carries the order: the importer writes files in path
/// order, and logs only append. Only messages and actions, the population
/// [`list_conversations`] counts.
async fn irc_messages(
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
    // `ORDER BY` takes no bound parameter, hence one literal per direction.
    // dev-lint: allow-sqlx — `sql` is one of the three literals directly above.
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
        let body: Option<String> = r.try_get("body")?;
        msgs.push(Message {
            id: id.to_string(),
            ts: s_to_ms(ts_s),
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            is_outgoing: is_self != 0,
            kind,
            body,
            deleted: false,
            edited: false,
            reactions: Vec::new(),
            edits: Vec::new(),
            link_images: Vec::new(),
            link_offers: Vec::new(),
            reply_to: None,
            delivery: None,
            entities: Vec::new(),
            album: None,
            previews: Vec::new(),
            attachments: Vec::new(),
        });
    }

    Ok(Fetched::new(msgs, keys, dir))
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct SearchHit {
    pub origin: Origin,
    pub conversation_id: String,
    pub conversation_name: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ts: i64,
    pub sender: String,
    pub snippet: String,
    /// The message was retracted; the reader hides the snippet behind a click.
    pub deleted: bool,
    /// Where the hit is: [`encode_cursor`] over the origin's native `(ts, id)`,
    /// passed unchanged to `messages_page`. `ts` is milliseconds, which cannot
    /// address a Google Chat row or tell IRC lines within a minute apart.
    pub cursor: String,
}

/// One page of a Telegram conversation. `sent_at` is in seconds, so `id` breaks
/// ties at every page boundary.
async fn telegram_messages(
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
                     m.sender_name AS sender, m.is_outgoing AS is_outgoing,
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
                     m.sender_name AS sender, m.is_outgoing AS is_outgoing,
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
                     m.sender_name AS sender, m.is_outgoing AS is_outgoing,
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
    // dev-lint: allow-sqlx — `sql` is one of the three literals directly above.
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
            // The row's id, unique across conversations; `msg_id` is not.
            id: id.to_string(),
            ts: s_to_ms(sent_at),
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            is_outgoing: is_outgoing != 0,
            // Service events render as actions.
            kind: if is_service {
                MessageKind::Action
            } else {
                MessageKind::Message
            },
            body,
            deleted: deleted != 0,
            edited,
            reactions: Vec::new(),
            attachments: Vec::new(),
            link_images: Vec::new(),
            edits: Vec::new(),
            link_offers: Vec::new(),
            reply_to: None,
            delivery: None,
            entities: Vec::new(),
            album: r
                .try_get::<Option<i64>, _>("grouped_id")?
                .map(|g| g.to_string()),
            previews: Vec::new(),
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
        // A fixed template with a computed count of `?` and every value bound.
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
        // A fixed template with a computed count of `?` and every value bound.
        let mut q = sqlx::query(AssertSqlSafe(sql)).bind(conversation_id);
        for id in &msg_ids {
            q = q.bind(id);
        }
        let named = telegram_reactors(pool, conversation_id, &msg_ids).await?;
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

    attach_telegram_replies(pool, conversation_id, &mut msgs, &replies).await?;
    attach_telegram_read(pool, conversation_id, &mut msgs, &msg_ids).await?;
    attach_telegram_entities(pool, conversation_id, &mut msgs, &msg_ids).await?;

    Ok(Fetched::new(msgs, keys, dir))
}

/// Telegram read state for this page's outgoing messages. The mark is one
/// high-water `msg_id` per conversation (`outbox`: how far they have read mine),
/// so this is a comparison. No mark leaves every message `None`.
async fn attach_telegram_read(
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
async fn attach_telegram_entities(
    pool: &MySqlPool,
    conversation_id: i64,
    msgs: &mut [Message],
    msg_ids: &[i32],
) -> Result<()> {
    if msg_ids.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; msg_ids.len()].join(",");
    // A fixed template with a computed count of `?` and every value bound.
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
async fn attach_telegram_edits(
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
    // A fixed template with a computed count of `?` and every value bound.
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

/// Where a search looks: everywhere, or inside one conversation. The scope is
/// in the SQL, not a filter after `LIMIT`, and origins it cannot match are not
/// queried.
#[derive(Debug, Clone, Copy)]
pub enum SearchScope<'a> {
    Everywhere,
    Conversation { origin: Origin, id: &'a str },
}

impl SearchScope<'_> {
    /// Whether this origin has anything to contribute to the result.
    fn covers(&self, origin: Origin) -> bool {
        match self {
            SearchScope::Everywhere => true,
            SearchScope::Conversation { origin: o, .. } => *o == origin,
        }
    }

    /// The conversation id, when this origin is the scoped one.
    fn id_for(&self, origin: Origin) -> Option<&str> {
        match self {
            SearchScope::Everywhere => None,
            SearchScope::Conversation { origin: o, id } if *o == origin => Some(id),
            SearchScope::Conversation { .. } => None,
        }
    }
}

/// Substring search across all origins, newest first. Retracted messages match;
/// the reader hides them as a thread does.
pub async fn search(
    pool: &MySqlPool,
    q: &str,
    limit: i64,
    scope: SearchScope<'_>,
) -> Result<Vec<SearchHit>> {
    let like = escape_like(q);
    let mut hits = Vec::new();

    // One literal per scope, not a built string; likewise for each origin below.
    //
    // A match in an edit's revision row is reported as the message it revises,
    // where the thread shows it, and a message matching in several versions is
    // one hit: the newest.
    let sql = if scope.id_for(Origin::Signal).is_some() {
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
    let srows = if scope.covers(Origin::Signal) {
        // dev-lint: allow-sqlx — `sql` is one of the two literals directly above.
        let mut q = sqlx::query(sql).bind(&like);
        if let Some(id) = scope.id_for(Origin::Signal) {
            q = q.bind(id);
        }
        q.bind(limit).fetch_all(pool).await?
    } else {
        Vec::new()
    };
    let mut seen = std::collections::HashSet::new();
    for r in srows {
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

    let sql = if scope.id_for(Origin::Gchat).is_some() {
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
    let grows = if scope.covers(Origin::Gchat) {
        // dev-lint: allow-sqlx — `sql` is one of the two literals directly above.
        let mut q = sqlx::query(sql).bind(&like);
        if let Some(id) = scope.id_for(Origin::Gchat) {
            q = q.bind(id);
        }
        q.bind(limit).fetch_all(pool).await?
    } else {
        Vec::new()
    };
    for r in grows {
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

    // Telegram: only what was said.
    let sql = if scope.id_for(Origin::Telegram).is_some() {
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
    let trows = if scope.covers(Origin::Telegram) {
        // dev-lint: allow-sqlx — `sql` is one of the two literals directly above.
        let mut q = sqlx::query(sql).bind(&like);
        // Bound as a string against a BIGINT: a malformed id matches nothing
        // rather than failing.
        if let Some(id) = scope.id_for(Origin::Telegram) {
            q = q.bind(id);
        }
        q.bind(limit).fetch_all(pool).await?
    } else {
        Vec::new()
    };
    for r in trows {
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

    // IRC: messages and actions only. `LIKE '%term%'` cannot use an index, so
    // the global form scans every line; the scoped form reads one conversation
    // through the `conversation_id` index. Cost follows match density: `LIMIT`
    // stops the scan early when matches are common.
    //
    // The scan runs alone in a derived table and is joined afterwards. Joined
    // directly, the optimizer drives from `irc_conversations` into random reads
    // of `irc_messages`. `is_status` is a subquery inside, so it filters before
    // `LIMIT` rather than shortening the page after it.
    let sql = if scope.id_for(Origin::Irc).is_some() {
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
    let irows = if scope.covers(Origin::Irc) {
        // dev-lint: allow-sqlx — `sql` is one of the two literals directly above.
        let mut q = sqlx::query(sql);
        if let Some(id) = scope.id_for(Origin::Irc) {
            q = q.bind(id);
        }
        q.bind(&like).bind(limit).fetch_all(pool).await?
    } else {
        Vec::new()
    };
    for r in irows {
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

    hits.sort_by_key(|h| std::cmp::Reverse(h.ts));
    hits.truncate(limit as usize);
    Ok(hits)
}
