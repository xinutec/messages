//! Read-only queries over the message archive, normalising three origins —
//! Signal, Google Chat and IRC — into one shape for the UI.
//!
//! They differ underneath: Signal keeps per-author reaction *events* (add/remove)
//! and edit/delete flags, Google Chat keeps *aggregated* emoji counts, its own
//! threading and a numeric sender id, IRC is flat lines carrying a `kind`. A
//! common `Conversation` / `Message` hides that from the frontend.
//!
//! ⚠ Everything here is SELECT-only, but the app is not: [`crate::irc_send`]
//! writes one row, for a message it has just sent through irssi. It lives there
//! because it is part of sending rather than reading.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::collections::HashMap;

use crate::link_image::{LinkState, askable};
use sqlx::{AssertSqlSafe, MySqlPool, Row};

/// Which archive a conversation came from.
///
/// One type for the URL path segment, the `origin` field the frontend reads and
/// every per-origin match arm, replacing the `"signal"`/`"gchat"` strings those
/// used to agree on by convention. The match in [`messages_page`] is exhaustive,
/// so adding an origin is a compile error at every site that has to handle it
/// rather than a silently empty page — which is how Telegram, the fourth, was
/// added without a page anywhere coming back blank.
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
    /// Parse the `{origin}` URL segment; None for anything else, which the API
    /// turns into a 404 (no such conversation, rather than a malformed request).
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

/// Whether a conversation is one-to-one, a group, or a broadcast.
///
/// The first two are the distinction the writer calls `ThreadKind` (see the
/// `signal` repo's `parse.rs`) and the `conversations.type` ENUM stores; named for
/// the reader's model, where it is a field of [`Conversation`] and where "thread"
/// already means Google Chat's in-group threading.
///
/// `channel` is Telegram's alone and is not a conversation at all — it is a feed
/// with an audience. It is stored rather than filtered because whether to show one
/// is the reader's question, and it is a THIRD value rather than folded into
/// `group` because a reader that wants people, not announcements, has no way back
/// once they are the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub enum ConversationKind {
    Dm,
    Group,
    /// A broadcast with an audience rather than a conversation. Telegram's only:
    /// the other three origins have no such thing.
    Channel,
}

impl ConversationKind {
    /// Parse a conversation-kind ENUM value. Signal's column is
    /// `ENUM('dm','group')` and Telegram's adds `channel`, so None means the
    /// schema moved underneath us — the caller errors rather than guessing a kind,
    /// which would mislabel every conversation of the new sort as a DM.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "dm" => Some(ConversationKind::Dm),
            "group" => Some(ConversationKind::Group),
            "channel" => Some(ConversationKind::Channel),
            _ => None,
        }
    }
}

// ⚠ This doc REACHES TYPESCRIPT — ts_rs copies it into `generated/`. So no
// intra-doc links (`[Foo::Bar]` renders as literal brackets there) and nothing
// that only makes sense to a Rust reader; those go in `//` like this.
/// Whether a line was said or done.
///
/// ⚠ **Two variants, and the IRC table has four.** Its column is
/// `ENUM('message','action','event','notice')`, but every query restricts to
/// message and action — joins, parts and server notices are not conversation.
/// Widening this would be claiming the reader shows things it does not. Signal
/// and Google Chat draw no such distinction and are always `message`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub enum MessageKind {
    /// Someone said something.
    Message,
    /// Someone did something — written in the third person about its sender,
    /// which is why every IRC client draws it `* nick waves` rather than
    /// `nick: waves`.
    Action,
}

impl MessageKind {
    /// Parse an `irc_messages.kind` ENUM value, for the two the queries admit.
    /// None means a row of a kind the filter should have excluded, which the
    /// caller reports rather than drawing as speech.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "message" => Some(MessageKind::Message),
            "action" => Some(MessageKind::Action),
            _ => None,
        }
    }
}

/// Telegram stores unix SECONDS — its own unit, and all it gives: the message
/// constructor has no sub-second field. The unified API uses milliseconds.
pub fn s_to_ms(s: i64) -> i64 {
    s * 1_000
}

/// Google Chat stores microsecond timestamps; the unified API uses milliseconds.
pub fn us_to_ms(us: i64) -> i64 {
    us / 1000
}

/// Google Chat stores no kind at all, only a boolean, so the two-valued enum
/// the reader works in is DERIVED here rather than read from a column. Signal
/// and IRC both carry theirs, which is why only this origin needs a function.
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct Conversation {
    pub origin: Origin,
    pub id: String,
    pub name: Option<String>,
    pub kind: ConversationKind,
    /// The IRC network, and None for the other two origins, which have no such
    /// thing. Sent because `name` is only the target: two networks each with an
    /// `s_20` produce two rows a reader cannot tell apart, which is what this is
    /// here to fix. Uniqueness is the schema's — `UNIQUE (network, target)`.
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
}

/// Whether an attachment's bytes can be ASKED for, and whether they have been.
///
/// ⚠ `None` is Signal's case and means "there is nothing to ask" — its blobs are
/// fetched by the ingester as they arrive, so an absent one is absent for good.
/// Telegram's large media is the case this exists for: the archive knows the file is
/// there and has deliberately not fetched it.
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
    /// Parse a `telegram_media.state` value. `stored` is deliberately absent: a
    /// stored file is described by `available`, and giving it a second
    /// representation here would let the two disagree.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "offered" => Some(FetchState::Offered),
            "wanted" => Some(FetchState::Wanted),
            "failed" => Some(FetchState::Failed),
            _ => None,
        }
    }
}

/// The two fields of an attachment that change while a fetch is in flight.
///
/// Deliberately the same shape those fields have on `Attachment`, so the reader
/// applies an answer by copying rather than by translating between two vocabularies.
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
    /// Whether the bytes are present (downloaded to the PVC). Metadata-only
    /// history rows are `false` — the UI shows them but can't fetch the blob.
    pub available: bool,
    pub is_image: bool,
    /// Whether these bytes can be asked for — `None` when there is nothing to ask.
    pub fetch: Option<FetchState>,
}

/// One version of a message that was edited, oldest first. The CURRENT text is
/// the message's own `body`; these are what it said before.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct MessageEdit {
    /// When this version was sent. Epoch milliseconds.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ts: i64,
    pub body: Option<String>,
}

/// A picture we hold for a link somebody posted — see `link_image.rs`. The UI
/// renders it under the message and links out to where it came from, so the
/// reader can still see whose it is.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct LinkImage {
    /// The link as it appears in the message — what the picture links out to.
    pub url: String,
    /// Our handle for the bytes: `/api/link-images/{id}`.
    pub id: String,
    pub content_type: String,
}

#[derive(Serialize, Clone)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
/// What a message was a reply TO, for the two origins that record one.
///
/// ⚠ **A reply can point at a message this archive does not hold**, and that is
/// not an error to hide: Signal's quote names a TIMESTAMP, so a quote of
/// anything older than the archive resolves to nothing, and Telegram's names a
/// message id that may sit in a gap the backfill has not reached. Every field
/// but the flag is therefore optional, and an unresolved reply still renders —
/// "this answered something" is true and worth showing even when the something
/// cannot be produced.
pub struct ReplyTo {
    /// The API id of the message replied to, or `None` when the archive does
    /// not hold it. Its presence is what makes the quote clickable.
    pub id: Option<String>,
    /// Cursor addressing the target, to be passed back as `?at` — the same
    /// landing a search hit uses. `None` exactly when `id` is.
    pub cursor: Option<String>,
    /// Epoch milliseconds of the message replied to, when known.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub ts: Option<i64>,
    pub sender: Option<String>,
    /// A short prefix of what it said — `None` for a target with no text, and
    /// for a DELETED one.
    pub excerpt: Option<String>,
    /// The target is held but deleted. ⚠ A deleted message's words stay behind a
    /// click in this app, so its excerpt is withheld here for the same reason:
    /// a quote is not the place that reveals it.
    pub deleted: bool,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct Message {
    pub id: String,
    /// Epoch milliseconds (Google Chat's native µs are converted on the way out).
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
    /// What this message said BEFORE it was edited, oldest first — empty unless
    /// it was. `body` is always the current text.
    pub edits: Vec<MessageEdit>,
    /// Links in `body` we could fetch a picture for but have not. Serving them
    /// offers them; a reader has to ask.
    pub link_offers: Vec<LinkOffer>,
    /// What this message answered, for the origins that record it — Signal and
    /// Telegram. Always `None` for Google Chat and IRC, neither of which has
    /// the association at all.
    pub reply_to: Option<ReplyTo>,
}

/// A link the reader can ask us to fetch a picture for.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct LinkOffer {
    /// The link as typed — what the control is offering to show.
    pub url: String,
    /// Its handle: `POST /api/link-images/{id}/request`, which answers with what
    /// the link turned out to be.
    pub id: String,
}

fn is_image(ct: Option<&str>) -> bool {
    ct.is_some_and(|c| c.starts_with("image/"))
}

/// One stored version of a message: when it was sent, and what it said.
type Version = (i64, Option<String>);

/// Put an edited message's history on it, and its CURRENT text in its body.
///
/// ⚠ **AN EDIT IS A SEPARATE ROW, AND UNTIL NOW BOTH WERE DRAWN.** Signal sends a
/// revision as a new message carrying `edit_of_ts`, so a thread showed the same
/// message twice — the old text where it was said, the new text minutes later,
/// with nothing saying they were one thing. 254 messages in the archive are in
/// that state. The revisions are excluded from the page above; this hangs them on
/// the message they revise.
///
/// ⚠ **The BODY becomes the newest version, and the position stays the original's.**
/// That is what an edit means: the thing was said then, and now reads this way.
/// Keeping the original's text as the body would show a message the sender has
/// already corrected, which is the failure this whole change is about.
///
/// One query for the page, like reactions.
/// How much of a quoted message a reply preview carries.
pub const EXCERPT_CHARS: usize = 120;

/// A one-line prefix of a quoted message.
///
/// ⚠ **Truncated by CHARACTERS, not bytes.** These bodies are full of emoji and
/// non-Latin text — the Telegram archive alone is mostly neither — and slicing a
/// `String` at a byte offset panics mid-codepoint. Newlines are folded because
/// the preview is one line by construction; letting a body's own line breaks
/// through would let a two-word quote push the message it belongs to off screen.
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

/// Resolve Signal's quotes for one page.
///
/// ⚠ **Signal names the quoted message by TIMESTAMP** (`quote_target_ts`), not by
/// id, so resolution is a lookup on `(thread_id, server_ts)` and legitimately
/// misses: a quote of anything older than this archive has nothing to match. A
/// miss is recorded as an unresolved reply rather than dropped — see [`ReplyTo`].
async fn attach_signal_replies(
    pool: &MySqlPool,
    thread_id: &str,
    msgs: &mut [Message],
    quotes: &[(String, i64)],
) -> Result<()> {
    if quotes.is_empty() {
        return Ok(());
    }
    let targets: Vec<i64> = {
        let mut t: Vec<i64> = quotes.iter().map(|(_, ts)| *ts).collect();
        t.sort_unstable();
        t.dedup();
        t
    };
    let placeholders = vec!["?"; targets.len()].join(",");
    let sql = format!(
        "SELECT m.id AS id, m.server_ts AS ts, m.body AS body, m.deleted AS deleted,
                COALESCE(ct.profile_name, m.sender_uuid) AS sender
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
    for (msg_id, target_ts) in quotes {
        let Some(m) = msgs.iter_mut().find(|m| &m.id == msg_id) else {
            continue;
        };
        m.reply_to = Some(found.get(target_ts).cloned().unwrap_or(ReplyTo {
            id: None,
            cursor: None,
            // ⚠ Kept even when the target is missing: for Signal the timestamp IS
            // the quote, so "answering something from 2024" stays sayable when
            // the something itself is not held.
            ts: Some(*target_ts),
            sender: None,
            excerpt: None,
            deleted: false,
        }));
    }
    Ok(())
}

/// Resolve Telegram's replies for one page.
///
/// ⚠ **Restricted to `kind = 'message'`, which is what the page query returns.**
/// A reply can name a service event, and resolving to one would hand back an id
/// for a row no page ever contains — the reader would click a quote and land
/// beside it rather than on it. Treating that as unresolved keeps the promise
/// that an id in a [`ReplyTo`] is a message the reader can actually be taken to.
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
                m.deleted AS deleted, m.sender_name AS sender
         FROM telegram_messages m
         WHERE m.conversation_id = ? AND m.kind = 'message'
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
        let body: Option<String> = r.try_get("body")?;
        found.insert(
            msg_id,
            ReplyTo {
                id: Some(id.to_string()),
                // ⚠ The cursor's ts is Telegram's NATIVE unit (seconds), because
                // that is what the page query compares against. `ts` below is
                // milliseconds, because that is what the API speaks. Minting the
                // cursor from the millisecond value would address a row a
                // thousand-fold in the future and page from the wrong end.
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
            // Telegram's reply names an id, which says nothing about WHEN — so
            // unlike Signal's, an unresolved one has no timestamp to offer.
            ts: None,
            sender: None,
            excerpt: None,
            deleted: false,
        }));
    }
    Ok(())
}

async fn attach_edits(pool: &MySqlPool, thread_id: &str, msgs: &mut [Message]) -> Result<()> {
    let originals: Vec<i64> = msgs.iter().filter(|m| m.edited).map(|m| m.ts).collect();
    if originals.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; originals.len()].join(",");
    // ⚠ **SCOPED TO THE THREAD, and matching on the timestamp alone was a real
    // defect.** `edit_of_ts` is a SERVER TIMESTAMP, not a message id: two
    // conversations can hold messages sharing a millisecond, and a revision
    // matched across threads would show one conversation's text inside another's
    // history. Found by three parallel tests seeding the same timestamp in
    // different threads — a message came back carrying six versions of which
    // four were somebody else's.
    let sql = format!(
        "SELECT edit_of_ts, server_ts, body FROM messages
         WHERE thread_id = ? AND edit_of_ts IN ({placeholders})
         ORDER BY server_ts ASC",
    );
    // Fixed template + computed placeholder count, values bound — safe.
    let mut q = sqlx::query(AssertSqlSafe(sql)).bind(thread_id);
    for ts in &originals {
        q = q.bind(ts);
    }

    // Group first, assign after. Doing both in one pass looks shorter and gets
    // the timestamps wrong: each version was sent when the NEXT one had not
    // arrived yet, so a version's stamp comes from the row before it, not from
    // the row carrying its text.
    let mut revisions: Vec<(i64, Vec<Version>)> = Vec::new();
    for row in q.fetch_all(pool).await? {
        let of: i64 = row.try_get("edit_of_ts")?;
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
        // What it said before, in order: the original's text, then every
        // revision but the newest. Each is stamped with when IT was sent — the
        // original at the message's own time, a revision at its own.
        let older_bodies = std::iter::once(m.body.clone())
            .chain(list.iter().rev().skip(1).rev().map(|(_, b)| b.clone()));
        let stamps = std::iter::once(m.ts).chain(list.iter().rev().skip(1).rev().map(|(t, _)| *t));
        m.edits = stamps
            .zip(older_bodies)
            .map(|(ts, body)| MessageEdit { ts, body })
            .collect();
        // The body is what it says NOW.
        m.body = newest;
    }
    Ok(())
}

/// Hang any pictures we hold onto the messages whose text linked them.
///
/// One query for the page, like reactions: the links are read out of the bodies
/// we already have, so this costs a single round trip however many links a page
/// carries. Only `ok` rows are joined — a link we decided was not a picture, or
/// could not reach, is simply a link.
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
    // ⚠ **EVERY STATE, NOT JUST THE PICTURES.** Selecting only `ok` rows made a
    // link we had already decided against look unseen, so the page offered a
    // control for it — and tapping that control 404s, because a decided row is
    // rightly not resolvable to an address. A button that cannot work is worse
    // than no button.
    let sql = format!(
        "SELECT url_hash, state, content_type, decided_by FROM link_images
         WHERE url_hash IN ({placeholders})",
    );
    // Fixed template + computed placeholder count, values bound — safe.
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for h in &hashes {
        q = q.bind(*h);
    }
    // Parsed once, here at the boundary: everything below reasons about the
    // closed set rather than about strings.
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

    // Three outcomes per link, and the middle one is the feature:
    //   a picture we hold → render it
    //   never seen, or offered and not yet asked for → offer it
    //   decided against → nothing; it stays a link
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
            // Offered, unreachable last time, or decided by an older reader —
            // `link_image::askable` is the one place that says which, and the
            // request endpoint asks it too.
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

/// Register this page's links so they can be asked for later, WITHOUT asking for
/// anything.
///
/// ⚠ **THIS IS THE ONLY PLACE A URL ENTERS THE TABLE**, and that is what keeps
/// the tap from being an open proxy: the address comes off a message in the
/// archive, never off a request. A tap can then only promote a row that already
/// exists, by its hash.
///
/// `INSERT IGNORE`, so a link already offered, asked for, or decided is left
/// exactly as it is — serving a page must never undo a decision or re-queue a
/// fetch.
async fn offer(pool: &MySqlPool, links: &[(&str, &str)]) -> Result<()> {
    if links.is_empty() {
        return Ok(());
    }
    let rows = vec!["(?, ?, 'offered', NOW())"; links.len()].join(",");
    let sql =
        format!("INSERT IGNORE INTO link_images (url_hash, url, state, wanted_at) VALUES {rows}");
    // Fixed template + computed placeholder count, values bound — safe.
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for (hash, url) in links {
        q = q.bind(*hash).bind(*url);
    }
    q.execute(pool).await.context("offering links")?;
    Ok(())
}

/// The URL of a link we offered, if we did. This is what keeps a request from
/// naming an address: the caller hands us a hash, and the address comes back off
/// the row that serving a page created from the message's own text.
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

/// Where a link's stored bytes are, if we hold them — the serving endpoint's
/// half of `attach_link_images`.
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

/// Stored location + content-type for an attachment blob, if its bytes exist.
/// Used by the serving endpoint; returns None when unknown or metadata-only.
/// What the archive now holds for one Telegram message, for a reader watching a
/// fetch it asked for.
///
/// ⚠ **This exists because the request is ASYNCHRONOUS and the POST cannot answer.**
/// A link picture is fetched while the request is open, so its response carries the
/// outcome; a 1.5GB video is fetched by another process minutes later. Without
/// something to ask, the control that said "fetching…" said it forever — which it
/// did, on a file that had already arrived ninety seconds earlier.
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

/// A reader asked for a Telegram file the archive has not fetched.
///
/// ⚠ Only `offered` and `failed` move: a request against something already stored
/// would queue a re-download that overwrites a good file, and one against something
/// already `wanted` would restart its place in the queue every time a reader tapped
/// twice. Reports whether anything changed so the caller can tell those apart from a
/// real queueing.
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

/// The bytes the archive holds for a Telegram message, by the message's api id.
///
/// ⚠ Resolved through `telegram_messages` rather than taken from the client,
/// because the client holds a MESSAGE id and the file is keyed by
/// `(conversation, msg_id)` — and doing the join here is what stops a request for
/// one conversation reaching a file in another that happens to share a number.
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

/// Which way a page runs from its cursor.
///
/// ⚠ **The direction cannot be a bound parameter**: `ORDER BY` will not take
/// one, and multiplying the sort keys by ±1 to fake it costs the index on a
/// 401,794-row table. So each origin carries the comparison twice, once per
/// direction, and this enum is what chooses between them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageDir {
    /// Back in time, strictly before the cursor. The whole archive read this way
    /// until #1401, because the thread's window was anchored to the newest
    /// message and could only grow one way.
    Older,
    /// Forward in time, strictly after the cursor. What SCROLLING forward needs:
    /// the caller already holds the row the cursor names.
    Newer,
    /// The cursor's own row, and forward from there. What LANDING needs.
    ///
    /// ⚠ **Both other directions are strict, so a landing built from `Older` +
    /// `Newer` skips the row it is aimed at.** Shipped that way on 2026-09-08
    /// and found on a phone: the reader was put one message past the hit and the
    /// marker naming "the message you searched for" pointed at its neighbour.
    /// The two halves must meet AND overlap by exactly the one row between them.
    AtAndNewer,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct MessagesPage {
    /// Ascending by ts.
    pub messages: Vec<Message>,
    pub has_more: bool,
    /// Opaque cursor addressing the page's OLDEST row — where to continue
    /// backwards. Pass back as `?cursor` with `dir=older`.
    ///
    /// ⚠ Named for the direction the reader travels, NOT for the order the page
    /// was fetched in. Both cursors describe the page's own ends, so a forward
    /// page and a backward page over the same rows hand back the same pair.
    pub next_cursor: Option<String>,
    /// Opaque cursor addressing the page's NEWEST row — where to continue
    /// forwards. Pass back as `?cursor` with `dir=newer`.
    pub prev_cursor: Option<String>,
}

/// All conversations across all three origins, newest activity first.
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

    // Telegram. Restricted to `kind = 'message'` for the reason IRC restricts to
    // message-and-action: a service event ("X joined", a pinned notice) is not
    // something anybody said, and counting it would make a conversation look
    // busier than it was.
    //
    // ⚠ **AGGREGATE-THEN-JOIN, and that shape is the IRC lesson applied before it
    // has to be learned again.** The derived table lets MariaDB answer the whole
    // aggregate from `idx_tg_conv_kind_ts`; grouping the join instead makes it
    // choose the unique key and read every candidate row. There is no maintained
    // stats table here yet and at this archive's size there should not be — the
    // threshold where one became necessary for IRC was measured at 3.7M rows and
    // 1.29s, and `irc_conversation_stats` (signal's v11-v14) is the ready-made
    // answer if Telegram ever approaches it.
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

    // IRC. ⚠ THIS READS A MAINTAINED TABLE AND MUST NOT GO BACK TO AGGREGATING.
    // `irc_conversation_stats` holds one row per conversation, kept current by
    // triggers on `irc_messages` (signal's migrations v11-v14) — which is also
    // where `kind IN ('message','action')` now lives, so joins, parts and server
    // notices never reach the count. Both sides of this join are a few hundred
    // rows, so the landing no longer depends on the size of the archive.
    //
    // The history, because the obvious "improvement" is to inline the aggregate
    // again. On 3,683,670 rows: grouping the join 27s, with FORCE INDEX 3.3s,
    // aggregate-then-join 1.5s — which tripped the 1s slow-statement alert on
    // every landing, and had no query fix left. The `kind` filter sits on the
    // middle column of `idx_irc_conv_kind_ts` and defeats MariaDB's loose index
    // scan (431 rows in 1.4ms without it, 3,614,079 in 1.29s with), and the
    // UNION-of-equalities rewrite its documentation suggests measured WORSE at
    // 2.15s — two scans instead of one. 0.75s is the floor for counting by scan.
    //
    // ⚠ `TIMESTAMPDIFF` from the epoch, NOT `UNIX_TIMESTAMP`, which reinterprets
    // a DATETIME against the connection's `time_zone` — the same row would come
    // back an hour out depending on who asked.
    //
    // `is_status = 0` drops the pseudo-conversation irssi files server notices
    // into: named after your own nick, so it looks like a DM with yourself.
    //
    // ⚠ `COALESCE(s.cnt, 0)` and a NULL `last_s`: a conversation with no counted
    // line has NO stats row, not a zero row.
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
            last_ts: last_s.map(|s| s * 1000),
        });
    }

    out.sort_by_key(|c| std::cmp::Reverse(c.last_ts)); // newest activity first
    Ok(out)
}

/// Where an IRC conversation actually is, for the send path.
///
/// The URL carries a conversation id, and irssi needs a network and a target —
/// so this is the one place that translates between them. Doing it from the
/// database rather than from anything the client sent is the point: a request
/// cannot name a network and a nick, only a conversation that already exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrcTarget {
    /// The conversation's network, after any `--map` the importer applied.
    pub network: String,
    /// A nick, or a channel including its leading `#`.
    pub target: String,
    /// ⚠ irssi files server notices under your OWN nick, so that log looks
    /// exactly like a conversation with yourself and is nothing of the kind.
    /// The reader already hides it; the sender must refuse it, or "reply" to a
    /// server notice would message you as though you were somebody else.
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

/// One page of a conversation, oldest→newest, with reactions attached.
///
/// `cursor` is a position from a previous page — its `next_cursor` to continue
/// backwards, its `prev_cursor` to continue forwards — and `dir` says which.
/// None starts at the most recent page, which only makes sense with
/// [`PageDir::Older`] and is what the thread opens with.
///
/// Each fetcher returns its rows ASCENDING plus the native `(ts, id)` of each
/// end, whichever direction it read in. Minting happens here, from the page
/// rather than from the query, so the two cursors mean the same thing on every
/// page: `next_cursor` is the oldest row and `prev_cursor` the newest.
pub async fn messages_page(
    pool: &MySqlPool,
    origin: Origin,
    id: &str,
    cursor: Option<(i64, i64)>,
    limit: i64,
    dir: PageDir,
) -> Result<MessagesPage> {
    let page = match origin {
        Origin::Signal => signal_messages(pool, id, cursor, limit, dir).await?,
        Origin::Gchat => gchat_messages(pool, id, cursor, limit, dir).await?,
        Origin::Irc => irc_messages(pool, id, cursor, limit, dir).await?,
        Origin::Telegram => telegram_messages(pool, id, cursor, limit, dir).await?,
    };
    let mut page = page;
    // ⚠ **Edit history is per-origin because the two origins that have any store
    // it differently.** Signal appends a row per version and points it at the
    // original (`edit_of_ts`); Telegram mutates the message and the archive files
    // the superseded text beside it. One function cannot read both, and calling
    // Signal's against a Telegram thread id would quietly find nothing and report
    // no history for every edited message.
    match origin {
        Origin::Signal => attach_edits(pool, id, &mut page.msgs).await?,
        Origin::Telegram => attach_telegram_edits(pool, id, &mut page.msgs).await?,
        // Google Chat's export carries no revisions, and IRC has no such concept.
        Origin::Gchat | Origin::Irc => {}
    }
    attach_link_images(pool, &mut page.msgs).await?;
    let page = page;
    let has_more = page.msgs.len() as i64 == limit;
    Ok(MessagesPage {
        messages: page.msgs,
        has_more,
        next_cursor: page.oldest.map(|(ts, id)| encode_cursor(ts, id)),
        prev_cursor: page.newest.map(|(ts, id)| encode_cursor(ts, id)),
    })
}

/// What a per-origin fetcher hands back: the page ASCENDING, and the native
/// `(ts, id)` of its two ends. Ascending regardless of direction, so that
/// everything above this line is direction-blind — the alternative, letting the
/// order follow the query, put a `reverse()` at the caller that was correct for
/// exactly one of the two directions.
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
    // Tie-broken by id so a page boundary never splits a run of messages sharing
    // a server_ts. The first `?` (cur_ts) doubles as the "no cursor → whole
    // thread" guard, in both directions.
    let sql = match dir {
        PageDir::Older => {
            r"SELECT m.id AS id, m.server_ts AS ts,
                 COALESCE(ct.profile_name, m.sender_uuid) AS sender,
                 m.is_outgoing AS is_outgoing, m.body AS body,
                 m.deleted AS deleted, m.edited AS edited,
                 m.quote_target_ts AS quote_target_ts
          FROM messages m
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          WHERE m.thread_id = ?
            AND m.edit_of_ts IS NULL
            AND (? IS NULL OR m.server_ts < ? OR (m.server_ts = ? AND m.id < ?))
          ORDER BY m.server_ts DESC, m.id DESC
          LIMIT ?"
        }
        PageDir::Newer => {
            r"SELECT m.id AS id, m.server_ts AS ts,
                 COALESCE(ct.profile_name, m.sender_uuid) AS sender,
                 m.is_outgoing AS is_outgoing, m.body AS body,
                 m.deleted AS deleted, m.edited AS edited,
                 m.quote_target_ts AS quote_target_ts
          FROM messages m
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          WHERE m.thread_id = ?
            AND m.edit_of_ts IS NULL
            AND (? IS NULL OR m.server_ts > ? OR (m.server_ts = ? AND m.id > ?))
          ORDER BY m.server_ts ASC, m.id ASC
          LIMIT ?"
        }
        PageDir::AtAndNewer => {
            r"SELECT m.id AS id, m.server_ts AS ts,
                 COALESCE(ct.profile_name, m.sender_uuid) AS sender,
                 m.is_outgoing AS is_outgoing, m.body AS body,
                 m.deleted AS deleted, m.edited AS edited,
                 m.quote_target_ts AS quote_target_ts
          FROM messages m
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          WHERE m.thread_id = ?
            AND m.edit_of_ts IS NULL
            AND (? IS NULL OR m.server_ts > ? OR (m.server_ts = ? AND m.id >= ?))
          ORDER BY m.server_ts ASC, m.id ASC
          LIMIT ?"
        }
    };
    // Nothing is BUILT here. The rule guards against constructed SQL, and a
    // match over two constants keeps every property it is guarding; the reason
    // there are two is that `ORDER BY` will not take a bound parameter.
    // dev-lint: allow-sqlx — `sql` is one of the two literals directly above.
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
    let mut quotes: Vec<(String, i64)> = Vec::new();
    for r in rows {
        let id: i64 = r.try_get("id")?;
        let ts: i64 = r.try_get("ts")?;
        if let Some(target) = r.try_get::<Option<i64>, _>("quote_target_ts")? {
            quotes.push((id.to_string(), target));
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
            kind: MessageKind::Message, // Signal has no action
            body: r.try_get("body")?,
            deleted: deleted != 0,
            edited: edited != 0,
            reactions: Vec::new(),
            attachments: Vec::new(),
            edits: Vec::new(),
            link_images: Vec::new(), // both filled for the whole page in messages_page
            link_offers: Vec::new(),
            reply_to: None,
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
                // Signal's blobs arrive with the message or not at all; there is
                // nothing a reader could ask for.
                fetch: None,
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

    attach_signal_replies(pool, thread_id, &mut msgs, &quotes).await?;

    Ok(Fetched::new(msgs, keys, dir))
}

async fn gchat_messages(
    pool: &MySqlPool,
    group_id: &str,
    cursor: Option<(i64, i64)>,
    limit: i64,
    dir: PageDir,
) -> Result<Fetched> {
    // The cursor carries the native µs ts (not the ms the UI sees), so paging
    // never skips rows that share a millisecond; id tie-breaks an exact µs match.
    let (cur_ts, cur_id) = (cursor.map(|(ts, _)| ts), cursor.map(|(_, id)| id));
    let sql = match dir {
        PageDir::Older => {
            r"SELECT m.id AS id, m.ts_us AS ts_us, m.sender_name AS sender,
                 m.is_self AS is_self, m.text AS body
          FROM gchat_messages m
          WHERE m.group_id = ?
            AND (? IS NULL OR m.ts_us < ? OR (m.ts_us = ? AND m.id < ?))
          ORDER BY m.ts_us DESC, m.id DESC
          LIMIT ?"
        }
        PageDir::Newer => {
            r"SELECT m.id AS id, m.ts_us AS ts_us, m.sender_name AS sender,
                 m.is_self AS is_self, m.text AS body
          FROM gchat_messages m
          WHERE m.group_id = ?
            AND (? IS NULL OR m.ts_us > ? OR (m.ts_us = ? AND m.id > ?))
          ORDER BY m.ts_us ASC, m.id ASC
          LIMIT ?"
        }
        PageDir::AtAndNewer => {
            r"SELECT m.id AS id, m.ts_us AS ts_us, m.sender_name AS sender,
                 m.is_self AS is_self, m.text AS body
          FROM gchat_messages m
          WHERE m.group_id = ?
            AND (? IS NULL OR m.ts_us > ? OR (m.ts_us = ? AND m.id >= ?))
          ORDER BY m.ts_us ASC, m.id ASC
          LIMIT ?"
        }
    };
    // Nothing is BUILT here. The rule guards against constructed SQL, and a
    // match over two constants keeps every property it is guarding; the reason
    // there are two is that `ORDER BY` will not take a bound parameter.
    // dev-lint: allow-sqlx — `sql` is one of the two literals directly above.
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
    let mut ids = Vec::with_capacity(rows.len());
    let mut keys = Vec::with_capacity(rows.len());
    for r in rows {
        let id: i64 = r.try_get("id")?;
        let ts_us: i64 = r.try_get("ts_us")?;
        keys.push((ts_us, id));
        let is_self: i8 = r.try_get("is_self")?;
        ids.push(id);
        msgs.push(Message {
            id: id.to_string(),
            ts: us_to_ms(ts_us),
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            is_outgoing: is_self != 0,
            kind: MessageKind::Message, // nor does Google Chat
            body: r.try_get("body")?,
            deleted: false,
            edited: false,
            reactions: Vec::new(),
            attachments: Vec::new(), // Google Chat export carries no attachments
            edits: Vec::new(),
            link_images: Vec::new(), // both filled for the whole page in messages_page
            link_offers: Vec::new(),
            reply_to: None,
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

    Ok(Fetched::new(msgs, keys, dir))
}

/// One page of an IRC conversation.
///
/// ⚠ **The cursor's native unit is seconds, and it is the coarsest of the three
/// origins by a wide margin.** irssi's default `timestamp_format` is `%H:%M`, so
/// the source records no seconds at all and every line in a busy minute shares
/// one timestamp — `id` is not a tie-break here so much as the actual ordering.
/// It holds because the importer walks files in sorted path order and a log is
/// append-only, so row id within a conversation is file order is time order.
///
/// Joins, parts and server notices are excluded: see the note in
/// [`list_conversations`]. The consequence worth knowing is that
/// `message_count` there and the rows here are the same population, so a
/// conversation never claims more messages than it will show.
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
    // Nothing is BUILT here. The rule guards against constructed SQL, and a
    // match over two constants keeps every property it is guarding; the reason
    // there are two is that `ORDER BY` will not take a bound parameter.
    // dev-lint: allow-sqlx — `sql` is one of the two literals directly above.
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
        // The query filters to the two this parses, so an unknown value means
        // the filter and the enum have drifted apart — reported, not drawn as
        // speech, because an event rendered as a line somebody said is a lie
        // the reader cannot see through.
        let Some(kind) = MessageKind::parse(&kind) else {
            bail!("irc_messages.kind holds a value this query should have excluded: {kind:?}");
        };
        let body: Option<String> = r.try_get("body")?;
        msgs.push(Message {
            id: id.to_string(),
            ts: ts_s * 1000,
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            is_outgoing: is_self != 0,
            kind,
            body,
            deleted: false,
            edited: false,
            reactions: Vec::new(), // IRC has none
            edits: Vec::new(),
            link_images: Vec::new(), // both filled for the whole page in messages_page
            link_offers: Vec::new(),
            reply_to: None,
            attachments: Vec::new(), // nor these
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
    /// The message was retracted. The snippet still carries its text; the reader
    /// hides it behind a click, exactly as a thread hides a deleted body.
    pub deleted: bool,
    /// WHERE the hit is, in the same opaque form the pager already speaks —
    /// [`encode_cursor`] over this origin's NATIVE `(ts, id)`.
    ///
    /// ⚠ Not `ts`. A hit's `ts` is normalised to milliseconds for display, and
    /// milliseconds cannot address a Google Chat row (µs) or separate two IRC
    /// lines in one second — which is most of them, since irssi's default
    /// `timestamp_format` records no seconds at all. Nor a bare row id, which is
    /// meaningless without the ts it tie-breaks. This is the pair, and
    /// `messages_page` takes it unchanged.
    pub cursor: String,
}

/// Simple substring search across all three origins' message text. Newest first.
///
/// ⚠ **RETRACTED MESSAGES MATCH.** Signal's `deleted` rows were excluded here
/// until 2026-09-04 while a thread sent the same text and hid it behind a click:
/// one concept, two policies, chosen in two places, neither aware of the other.
/// Decided 2026-09-04 — a search that cannot find what was retracted is not an
/// archive's search, and the hit is hidden the way the thread hides a body. The
/// rule now lives in the reader for both, rather than half here and half there.
///
/// Only Signal has retraction at all: gchat and IRC have no such column, so their
/// hits are `deleted: false` because there is nothing to be deleted, not because
/// anything was checked.
/// One page of a Telegram conversation.
///
/// ⚠ The cursor carries `sent_at` in SECONDS, which is coarse: a busy minute puts
/// many messages on one value, so `id` is doing more tie-breaking work here than
/// in the other origins. That is why it is in the comparison at every boundary
/// rather than only in the `ORDER BY` — without it a page boundary that lands
/// inside a second would either repeat or skip whatever shares it.
async fn telegram_messages(
    pool: &MySqlPool,
    conversation_id: &str,
    cursor: Option<(i64, i64)>,
    limit: i64,
    dir: PageDir,
) -> Result<Fetched> {
    // A Telegram conversation id is a number in the URL. A non-numeric one is not
    // a conversation that can exist, so it pages as empty rather than erroring —
    // the same answer the other origins give for an id nothing matches.
    let Ok(conversation_id) = conversation_id.parse::<i64>() else {
        return Ok(Fetched::new(Vec::new(), Vec::new(), dir));
    };
    let (cur_ts, cur_id) = (cursor.map(|(ts, _)| ts), cursor.map(|(_, id)| id));
    // Only what was SAID: `kind = 'message'` leaves out the service events the
    // archive also holds, matching what the conversation list counts.
    let sql = match dir {
        PageDir::Older => {
            r"SELECT m.id AS id, m.msg_id AS msg_id, m.sent_at AS sent_at,
                     m.sender_name AS sender, m.is_outgoing AS is_outgoing,
                     m.text AS body, m.deleted AS deleted, m.edited_at AS edited_at,
                     m.edit_hidden AS edit_hidden,
                     m.reply_to_msg_id AS reply_to_msg_id
              FROM telegram_messages m
              WHERE m.conversation_id = ? AND m.kind = 'message'
                AND (? IS NULL OR m.sent_at < ? OR (m.sent_at = ? AND m.id < ?))
              ORDER BY m.sent_at DESC, m.id DESC
              LIMIT ?"
        }
        PageDir::Newer => {
            r"SELECT m.id AS id, m.msg_id AS msg_id, m.sent_at AS sent_at,
                     m.sender_name AS sender, m.is_outgoing AS is_outgoing,
                     m.text AS body, m.deleted AS deleted, m.edited_at AS edited_at,
                     m.edit_hidden AS edit_hidden,
                     m.reply_to_msg_id AS reply_to_msg_id
              FROM telegram_messages m
              WHERE m.conversation_id = ? AND m.kind = 'message'
                AND (? IS NULL OR m.sent_at > ? OR (m.sent_at = ? AND m.id > ?))
              ORDER BY m.sent_at ASC, m.id ASC
              LIMIT ?"
        }
        PageDir::AtAndNewer => {
            r"SELECT m.id AS id, m.msg_id AS msg_id, m.sent_at AS sent_at,
                     m.sender_name AS sender, m.is_outgoing AS is_outgoing,
                     m.text AS body, m.deleted AS deleted, m.edited_at AS edited_at,
                     m.edit_hidden AS edit_hidden,
                     m.reply_to_msg_id AS reply_to_msg_id
              FROM telegram_messages m
              WHERE m.conversation_id = ? AND m.kind = 'message'
                AND (? IS NULL OR m.sent_at > ? OR (m.sent_at = ? AND m.id >= ?))
              ORDER BY m.sent_at ASC, m.id ASC
              LIMIT ?"
        }
    };
    // Nothing is BUILT here: one of three literals above, values bound.
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
        // ⚠ **AN EDIT DATE IS NOT AN EDIT TO SHOW.** Telegram's `edit_hide` says
        // "the message should be shown as not modified to the user, even if an edit
        // date is present" — it sets a date for its own reasons and asks clients not
        // to surface it, which its own apps honour. This reader did not, and printed
        // `edited` on a photo Telegram showed as untouched.
        //
        // NULL means the archive has not learned the flag for that row yet (it
        // predates the column), and is read as "not hidden" — the behaviour from
        // before, which is the honest default for a row that was never asked.
        let edit_hidden: Option<i8> = r.try_get("edit_hidden")?;
        let edited = edited_at.is_some() && edit_hidden.unwrap_or(0) == 0;
        keys.push((sent_at, id));
        msg_ids.push(r.try_get::<i32, _>("msg_id")?);
        msgs.push(Message {
            // ⚠ The row's surrogate id, NOT `msg_id`. The API's message id has to
            // be unique across the page and stable for the reactions join below;
            // `msg_id` is unique only within its conversation, which is true here
            // but stops being true the moment anything holds two pages at once.
            id: id.to_string(),
            ts: s_to_ms(sent_at),
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            is_outgoing: is_outgoing != 0,
            // Telegram draws no action/message distinction; its service events are
            // a separate `kind` this query excludes.
            kind: MessageKind::Message,
            body: r.try_get("body")?,
            deleted: deleted != 0,
            edited,
            reactions: Vec::new(),
            // No bytes are stored for Telegram media, so there is nothing to serve
            // — see the v16 migration in the `signal` repo.
            attachments: Vec::new(),
            link_images: Vec::new(),
            edits: Vec::new(),
            link_offers: Vec::new(),
            reply_to: None,
        });
    }

    // Media this archive holds bytes for, as `attachments`.
    //
    // ⚠ **`attachments` WAS "SIGNAL ONLY", AND THAT WAS NEVER WHAT IT MEANT.** It
    // means "bytes this archive holds for this message", and Signal was simply the
    // only origin that had any. Giving Telegram a parallel field would have made one
    // concept two, with the copied-log namer, the is-image test and the not-stored
    // marker each needing a second implementation — the exact shape this repository's
    // README keeps a table about.
    //
    // `available = false` is the normal case for anything large: the file is at
    // Telegram and has not been fetched. The reader draws it as not stored, which is
    // true, and is the hook a request button will later hang from.
    if !msg_ids.is_empty() {
        let placeholders = vec!["?"; msg_ids.len()].join(",");
        // ⚠ The SIZE comes from `telegram_messages`, not from the media row, and the
        // join is why. `telegram_media` held its own `size_bytes` for a day and it
        // was a `stat` that raced the write's visibility — 218MB of files recorded
        // as 86MB, some as zero. One number, in the table whose subject is what
        // Telegram said about the file.
        let sql = format!(
            "SELECT d.msg_id AS msg_id, d.state AS state, d.content_type AS content_type,
                    m.media_size AS media_size
             FROM telegram_media d
             JOIN telegram_messages m
               ON m.conversation_id = d.conversation_id AND m.msg_id = d.msg_id
             WHERE d.conversation_id = ? AND d.msg_id IN ({placeholders})",
        );
        // Fixed template, computed placeholder count, every value bound.
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
            // The api id is read out before the mutable borrow, not through it.
            let api_id = msgs[i].id.clone();
            msgs[i].attachments.push(Attachment {
                // ⚠ The MESSAGE's api id, not the media row's: the route resolves a
                // Telegram file by the message it belongs to, because that is the
                // only id the client has in hand.
                id: api_id,
                is_image: is_image(content_type.as_deref()),
                content_type,
                // Telegram photos carry no filename. `attachment.ts` already names an
                // unnamed attachment from its type, on both the screen and the
                // clipboard, so `None` is the honest value rather than a synthesised one.
                file_name: None,
                size: mr.try_get("media_size")?,
                available: state == "stored",
                fetch: FetchState::parse(&state),
            });
        }
    }

    // Reactions, already aggregated per emoji by the writer — the same shape
    // Google Chat's have, so the same limit applies: you can see that four people
    // laughed, not which four.
    //
    // ⚠ A custom emoji has no characters to draw. Its row holds a document id and
    // a NULL emoji, and this leaves those out rather than rendering a blank bubble
    // with a count beside it. What the archive holds and what the screen can show
    // are different questions, and this is the second one.
    //
    // ⚠ **And `removed_at IS NULL`, which is the same distinction again.** A
    // reaction taken back KEEPS ITS ROW in the archive — the ingester dates it
    // instead of deleting it, so a re-walk cannot forget that it happened — and the
    // thread draws what is on the message NOW. Without this filter every retracted
    // reaction would come back the moment the archive learned it was gone.
    if !msg_ids.is_empty() {
        let placeholders = vec!["?"; msg_ids.len()].join(",");
        let sql = format!(
            "SELECT msg_id, emoji, cnt FROM telegram_reactions
             WHERE conversation_id = ? AND emoji IS NOT NULL
               AND removed_at IS NULL
               AND msg_id IN ({placeholders})",
        );
        // Fixed template, computed placeholder count, every value bound.
        let mut q = sqlx::query(AssertSqlSafe(sql)).bind(conversation_id);
        for id in &msg_ids {
            q = q.bind(id);
        }
        for rr in q.fetch_all(pool).await? {
            let msg_id: i32 = rr.try_get("msg_id")?;
            let reaction = Reaction {
                emoji: rr.try_get("emoji")?,
                count: i64::from(rr.try_get::<i32, _>("cnt")?),
            };
            // The page's rows in order, so position by `msg_id` rather than the
            // surrogate id the API reports.
            if let Some(i) = msg_ids.iter().position(|m| *m == msg_id) {
                msgs[i].reactions.push(reaction);
            }
        }
    }

    attach_telegram_replies(pool, conversation_id, &mut msgs, &replies).await?;

    Ok(Fetched::new(msgs, keys, dir))
}

/// Telegram's edit history: the superseded versions of each edited message.
///
/// ⚠ **The ordering is the whole difficulty, and it is not the one Signal has.**
/// Signal's versions each arrive with their own send time, so they sort by it.
/// Telegram gives a message one `edit_date` — the LAST edit — so a superseded
/// version is filed under the `edit_date` it carried, and the ORIGINAL carried
/// none. `was_edited_at IS NULL` is therefore the oldest version rather than an
/// unknown one, and sorting it as NULL-last would put the original at the end of
/// its own history.
/// ⚠ Driven by `m.edited`, which already accounts for `edit_hide` — so a message
/// Telegram asks us to show as unmodified gets no history panel either. That is one
/// statement rather than two decisions: the prior text stays in the archive, and the
/// reader simply does not offer it.
async fn attach_telegram_edits(
    pool: &MySqlPool,
    conversation_id: &str,
    msgs: &mut [Message],
) -> Result<()> {
    let Ok(conversation_id) = conversation_id.parse::<i64>() else {
        return Ok(());
    };
    // Only the edited ones, and by the id the API reported.
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
    // Fixed template, computed placeholder count, every value bound.
    let mut q = sqlx::query(AssertSqlSafe(sql)).bind(conversation_id);
    for id in &ids {
        q = q.bind(id);
    }
    for row in q.fetch_all(pool).await? {
        let row_id: i64 = row.try_get("row_id")?;
        let row_id = row_id.to_string();
        // The original version was sent when the message was sent; a later
        // superseded version was current until the edit that replaced it, which is
        // the date it carries.
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

pub async fn search(pool: &MySqlPool, q: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let like = escape_like(q);
    let mut hits = Vec::new();

    let srows = sqlx::query(
        r"SELECT m.id AS id, m.thread_id AS cid, c.name AS cname, m.server_ts AS ts,
                 COALESCE(ct.profile_name, m.sender_uuid) AS sender, m.body AS body,
                 m.deleted AS deleted
          FROM messages m
          LEFT JOIN conversations c ON c.thread_id = m.thread_id
          LEFT JOIN contacts ct ON ct.uuid = m.sender_uuid
          WHERE m.body LIKE ?
          ORDER BY m.server_ts DESC LIMIT ?",
    )
    .bind(&like)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    for r in srows {
        let deleted: i8 = r.try_get("deleted")?;
        let id: i64 = r.try_get("id")?;
        let ts: i64 = r.try_get("ts")?;
        hits.push(SearchHit {
            origin: Origin::Signal,
            conversation_id: r.try_get("cid")?,
            conversation_name: r.try_get("cname")?,
            ts,
            sender: r.try_get("sender")?,
            snippet: r.try_get::<Option<String>, _>("body")?.unwrap_or_default(),
            deleted: deleted != 0,
            // Signal's native ts IS milliseconds, so this pair happens to equal
            // the displayed `ts`. The other two do not, which is why the cursor
            // is minted per-origin rather than once from `ts`.
            cursor: encode_cursor(ts, id),
        });
    }

    let grows = sqlx::query(
        r"SELECT m.id AS id, m.group_id AS cid, g.name AS cname, m.ts_us AS ts_us,
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
            deleted: false, // Google Chat's export records no retraction
            // MICROSECONDS, deliberately unlike the `ts` above: two rows inside
            // one millisecond are distinct here and identical there.
            cursor: encode_cursor(ts_us, id),
        });
    }

    // Telegram. Only what was said, as the list and the page do, and the id is
    // numeric so the hit carries it as text the way the URL will.
    let trows = sqlx::query(
        r"SELECT m.id AS id, m.conversation_id AS cid, t.name AS cname,
                 m.sent_at AS sent_at, m.sender_name AS sender, m.text AS body,
                 m.deleted AS deleted
          FROM telegram_messages m
          LEFT JOIN telegram_conversations t ON t.id = m.conversation_id
          WHERE m.kind = 'message' AND m.text LIKE ?
          ORDER BY m.sent_at DESC LIMIT ?",
    )
    .bind(&like)
    .bind(limit)
    .fetch_all(pool)
    .await?;
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
            // ⚠ The snippet is sent even for a retracted message, and the reader
            // hides it — the archive-wide policy this repo settled on 2026-09-04.
            // A search that cannot find what was retracted is not an archive's
            // search.
            snippet: r.try_get::<Option<String>, _>("body")?.unwrap_or_default(),
            deleted: deleted != 0,
            // SECONDS, matching what `telegram_messages` pages on. Minting this
            // from `ts` would put milliseconds in a cursor the query compares
            // against seconds, and every landing would miss.
            cursor: encode_cursor(sent_at, id),
        });
    }

    // IRC. Searching only what was *said* — the same restriction the list and
    // the page use. Joins, parts and server notices would otherwise dominate
    // every result: 45% of the 3.69M lines (counted 2026-08-16), and they are
    // the ones full of words like "connection" and "user" that somebody
    // searching would actually type.
    //
    // ⚠ **THE SUBSTRING SCAN MUST NOT BE JOINED TO, and this is the same trap
    // the conversation list fell into.** Written as one flat join, the optimizer
    // drives from `irc_conversations` (315 rows) and reaches `irc_messages` by
    // `ref` — 3.7M secondary-index entries, each followed by a primary-key
    // lookup to read `text`, which is random I/O over a 502 MiB table behind a
    // 128 MiB buffer pool. MEASURED 2026-08-14: **32.4s**. Scanning
    // `irc_messages` alone and joining the surviving 50 rows afterwards is
    // **10.0s** for a result set proved identical (same count, same id bounds).
    //
    // `is_status` therefore moves inside as a subquery against the small table,
    // NOT to a filter after the join: applying it later would filter rows the
    // `LIMIT` had already chosen, and a page would silently come back short.
    //
    // ⚠ 10s is still not fast, and no rewrite will fix that — `LIKE '%term%'`
    // cannot use an index, so the floor is one pass over every row (a bare
    // `SUM(LENGTH(text))` is already 7.5s). Going below it means a FULLTEXT
    // index, which searches WORDS: `nix` would stop matching `nixos`. That is a
    // decision about what search means rather than how it runs, so it is
    // Pippijn's, and it is filed as **#882**.
    let irows = sqlx::query(
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
          ORDER BY m.sent_at DESC, m.id DESC",
    )
    .bind(&like)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    for r in irows {
        let cid: i32 = r.try_get("cid")?;
        let ts_s: i64 = r.try_get("ts_s")?;
        let id: i64 = r.try_get("id")?;
        hits.push(SearchHit {
            origin: Origin::Irc,
            conversation_id: cid.to_string(),
            conversation_name: r.try_get("cname")?,
            ts: ts_s * 1000,
            sender: r
                .try_get::<Option<String>, _>("sender")?
                .unwrap_or_default(),
            snippet: r.try_get::<Option<String>, _>("body")?.unwrap_or_default(),
            deleted: false, // IRC has no retraction
            // WHOLE SECONDS, the coarsest of the three: irssi's default format
            // records none, so `id` is the real ordering inside a minute and a
            // cursor without it addresses nothing.
            cursor: encode_cursor(ts_s, id),
        });
    }

    hits.sort_by_key(|h| std::cmp::Reverse(h.ts)); // newest first
    hits.truncate(limit as usize);
    Ok(hits)
}
