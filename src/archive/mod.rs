//! Read-only queries over the message archive, normalising Signal, Google Chat,
//! IRC and Telegram into one shape for the UI. The only write, the echo of a sent
//! IRC line, is in [`crate::irc_send`].
//!
//! One module per origin, each with its conversations, pages, search and stored
//! bytes; this module holds the shapes they share and the calls that combine them.
//!
//! A query over a page's rows is built with one `?` per value and every value
//! bound; that fixed shape is what each `AssertSqlSafe` asserts.

use anyhow::Result;
use serde::Serialize;
use sqlx::{MySqlPool, Row};

pub mod gchat;
pub mod irc;
pub mod links;
pub mod signal;
pub mod telegram;

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
fn us_to_ms(us: i64) -> i64 {
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
        Origin::Gchat => day_start_ms * 1000,
        Origin::Telegram | Origin::Irc => day_start_ms / 1000,
    };
    encode_cursor(native, 0)
}

/// Google Chat stores only `is_dm`, so its kind is derived.
fn kind_from_is_dm(is_dm: bool) -> ConversationKind {
    if is_dm {
        ConversationKind::Dm
    } else {
        ConversationKind::Group
    }
}

/// A search term as a SQL `LIKE` pattern, with `%` and `_` matching literally.
/// The result is still bound as a parameter.
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
    /// Its newest message, for the line under the name.
    pub last: Option<LastMessage>,
    /// Messages from others that came in after I last read, on the phone; 0 when
    /// the origin cannot say (Google Chat, IRC) or nothing is unread.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub unread: i64,
    /// When its picture last changed, in epoch seconds: the version its URL
    /// carries. `None` without a picture; see `avatars.rs`.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub avatar: Option<i64>,
}

/// A conversation's newest message, as the list shows it.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct LastMessage {
    pub sender: String,
    pub is_outgoing: bool,
    pub deleted: bool,
    /// One line of it; `None` when it has no text or was deleted.
    pub text: Option<String>,
    /// The content type of its first attachment, so a message of only a picture
    /// can say so; `None` when it carried none or was deleted.
    pub media: Option<String>,
}

impl LastMessage {
    /// From a list query's `last_sender`, `last_out`, `last_text` and `last_media` columns,
    /// `None` when the conversation has no message; a deleted one shows no text.
    fn from_row(r: &sqlx::mysql::MySqlRow, deleted: bool) -> Result<Option<Self>> {
        let Some(sender) = r.try_get::<Option<String>, _>("last_sender")? else {
            return Ok(None);
        };
        let out: i8 = r.try_get("last_out")?;
        let text: Option<String> = r.try_get("last_text")?;
        let media: Option<String> = r.try_get("last_media")?;
        Ok(Some(LastMessage {
            sender,
            is_outgoing: out != 0,
            deleted,
            text: if deleted {
                None
            } else {
                excerpt(text.as_deref())
            },
            media: if deleted { None } else { media },
        }))
    }
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
    /// `code`, `spoiler`, `mention`, … A string, so an unknown kind renders as
    /// plain text. Signal's styles and mentions are mapped onto these.
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

impl Message {
    /// A message with nothing attached yet; each origin's page fills in the rest.
    fn bare(id: i64, ts: i64, sender: String, is_outgoing: bool, body: Option<String>) -> Self {
        Message {
            id: id.to_string(),
            ts,
            sender,
            is_outgoing,
            kind: MessageKind::Message,
            body,
            deleted: false,
            edited: false,
            reactions: Vec::new(),
            attachments: Vec::new(),
            link_images: Vec::new(),
            edits: Vec::new(),
            link_offers: Vec::new(),
            reply_to: None,
            delivery: None,
            entities: Vec::new(),
            album: None,
            previews: Vec::new(),
        }
    }
}

impl ReplyTo {
    /// A reply whose target the archive does not hold, and nothing else known.
    fn not_held() -> Self {
        ReplyTo {
            id: None,
            cursor: None,
            ts: None,
            sender: None,
            excerpt: None,
            deleted: false,
        }
    }
}

/// A link preview as the sender's app made it.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct LinkPreview {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// Its handle for `GET /api/link-previews/{id}/image`, when the archive
    /// holds the picture the sender's app fetched for it.
    pub image: Option<String>,
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

/// Each value once, sorted: the list a page's `IN (…)` asks for.
fn distinct<T: Ord>(values: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut v: Vec<T> = values.into_iter().collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn is_image(ct: Option<&str>) -> bool {
    ct.is_some_and(|c| c.starts_with("image/"))
}

/// How much of a quoted message a reply preview carries.
pub const EXCERPT_CHARS: usize = 120;

/// A one-line prefix of a quoted message.
///
/// Cut by characters: a byte slice panics mid-codepoint.
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

/// All conversations across all origins, newest activity first.
pub async fn list_conversations(pool: &MySqlPool) -> Result<Vec<Conversation>> {
    let mut out = signal::conversations(pool).await?;
    out.extend(gchat::conversations(pool).await?);
    out.extend(telegram::conversations(pool).await?);
    out.extend(irc::conversations(pool).await?);
    out.sort_by_key(|c| std::cmp::Reverse(c.last_ts));
    Ok(out)
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
        Origin::Signal => signal::page(pool, id, cursor, limit, dir).await?,
        Origin::Gchat => gchat::page(pool, id, cursor, limit, dir).await?,
        Origin::Irc => irc::page(pool, id, cursor, limit, dir).await?,
        Origin::Telegram => telegram::page(pool, id, cursor, limit, dir).await?,
    };
    // Signal stores an edit as a new row pointing at the original; Telegram
    // edits in place and files the old text beside it.
    match origin {
        Origin::Signal => {
            let shown = signal::attach_edits(pool, id, &mut page.msgs).await?;
            signal::attach_styles(pool, &mut page.msgs, &shown).await?;
            signal::attach_mentions(pool, &mut page.msgs, &shown).await?;
            signal::attach_previews(pool, &mut page.msgs).await?;
        }
        Origin::Telegram => telegram::attach_edits(pool, id, &mut page.msgs).await?,
        // Neither has revisions.
        Origin::Gchat | Origin::Irc => {}
    }
    links::attach(pool, &mut page.msgs).await?;
    let has_more = page.msgs.len() as i64 == limit;
    Ok(MessagesPage {
        messages: page.msgs,
        has_more,
        next_cursor: page.oldest.map(|(ts, id)| encode_cursor(ts, id)),
        prev_cursor: page.newest.map(|(ts, id)| encode_cursor(ts, id)),
    })
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
    if scope.covers(Origin::Signal) {
        hits.extend(signal::search(pool, &like, scope.id_for(Origin::Signal), limit).await?);
    }
    if scope.covers(Origin::Gchat) {
        hits.extend(gchat::search(pool, &like, scope.id_for(Origin::Gchat), limit).await?);
    }
    if scope.covers(Origin::Telegram) {
        hits.extend(telegram::search(pool, &like, scope.id_for(Origin::Telegram), limit).await?);
    }
    if scope.covers(Origin::Irc) {
        hits.extend(irc::search(pool, &like, scope.id_for(Origin::Irc), limit).await?);
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.ts));
    hits.truncate(limit as usize);
    Ok(hits)
}
