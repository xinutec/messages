//! Tests for the archive query/normalisation layer.
//!
//! Pure unit tests (timestamp/kind/LIKE-escape) always run. The end-to-end DB
//! tests seed a known fixture into a MariaDB and assert the real queries —
//! ordering, the `before` pagination cursor, Signal reaction aggregation, edit/
//! delete flags, the µs→ms conversion, and cross-origin search. They run when
//! `MESSAGES_TEST_DATABASE_URL` points at a *throwaway* database (the test
//! drops+recreates the archive tables), and are skipped otherwise. CI sets it
//! from a MariaDB service; locally the gate's `tests` row runs the suite through
//! `dev-lint`'s `with-test-db`, which starts one, so the pre-commit gate covers
//! them too. NEVER point it at the real signal DB.

use messages::archive::{
    self, ConversationKind, Origin, encode_cursor, escape_like, kind_from_is_dm, parse_cursor,
    us_to_ms,
};

// ---- pure units (no DB) -----------------------------------------------------

#[test]
fn us_to_ms_truncates_to_millis() {
    assert_eq!(us_to_ms(7_000_000), 7000);
    assert_eq!(us_to_ms(1_584_389_732_190_514), 1_584_389_732_190);
}

#[test]
fn kind_from_is_dm_maps_both() {
    assert_eq!(kind_from_is_dm(true), ConversationKind::Dm);
    assert_eq!(kind_from_is_dm(false), ConversationKind::Group);
}

#[test]
fn conversation_kind_parses_the_enum_column_and_nothing_else() {
    assert_eq!(ConversationKind::parse("dm"), Some(ConversationKind::Dm));
    assert_eq!(
        ConversationKind::parse("group"),
        Some(ConversationKind::Group)
    );
    // The column is ENUM('dm','group'); anything else means the schema moved,
    // and list_conversations errors rather than defaulting to a kind.
    assert_eq!(ConversationKind::parse("channel"), None);
    assert_eq!(ConversationKind::parse("DM"), None);
}

/// The wire spelling is the frontend's contract — `Origin` and `ConversationKind`
/// are string unions in the generated TS, so a renamed variant would silently
/// change the JSON. Serialised here so that change fails a test instead.
#[test]
fn enums_serialise_to_the_spellings_the_frontend_expects() {
    assert_eq!(
        serde_json::to_string(&Origin::Signal).unwrap(),
        r#""signal""#
    );
    assert_eq!(serde_json::to_string(&Origin::Gchat).unwrap(), r#""gchat""#);
    assert_eq!(serde_json::to_string(&Origin::Irc).unwrap(), r#""irc""#);
    assert_eq!(
        serde_json::to_string(&ConversationKind::Dm).unwrap(),
        r#""dm""#
    );
    assert_eq!(
        serde_json::to_string(&ConversationKind::Group).unwrap(),
        r#""group""#
    );
}

#[test]
fn escape_like_neutralises_wildcards() {
    assert_eq!(escape_like("hi"), "%hi%");
    assert_eq!(escape_like("a%b_c"), "%a\\%b\\_c%");
    assert_eq!(escape_like("back\\slash"), "%back\\\\slash%");
}

#[test]
fn origin_only_parses_known_path_segments() {
    assert_eq!(Origin::parse("signal"), Some(Origin::Signal));
    assert_eq!(Origin::parse("gchat"), Some(Origin::Gchat));
    assert_eq!(Origin::parse("irc"), Some(Origin::Irc));
    assert_eq!(Origin::parse("email"), None);
    assert_eq!(Origin::parse(""), None);
}

#[test]
fn cursor_round_trips_and_rejects_garbage() {
    assert_eq!(
        parse_cursor(&encode_cursor(1_717_000_000_000, 42)),
        Some((1_717_000_000_000, 42))
    );
    // Malformed → None, so the API just falls back to the newest page.
    assert_eq!(parse_cursor("nope"), None);
    assert_eq!(parse_cursor("123_"), None);
    assert_eq!(parse_cursor("_9"), None);
    assert_eq!(parse_cursor(""), None);
}

// ---- end-to-end against a real MariaDB --------------------------------------

use sqlx::mysql::MySqlPoolOptions;
use sqlx::{AssertSqlSafe, MySqlPool};

async fn test_pool() -> Option<MySqlPool> {
    let url = std::env::var("MESSAGES_TEST_DATABASE_URL").ok()?;
    let pool = MySqlPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect to MESSAGES_TEST_DATABASE_URL");
    Some(pool)
}

/// The fixture is seeded once per process, not once per test.
///
/// `seed` DROPs and recreates the archive tables, so seeding per test races when
/// the tests run in parallel against the one database — each dropping the tables
/// another has just filled. That was survivable only because the single place
/// these tests ran, CI, passed `--test-threads=1`; the moment the local gate ran
/// them the way cargo runs tests by default, five of six failed on the DDL.
///
/// Seeding once removes the need for that flag, so CI and the local gate can run
/// the suite the same way — a race is worth catching wherever it appears, not
/// suppressing in the one environment that had learned to avoid it.
///
/// ⚠ THIS IS NO LONGER A READ-ONLY SUITE, and the rule that replaced "nothing
/// writes" is narrower: a test that writes must confine itself to rows no other
/// test asserts on. The send-path tests archive an echo into `irc_messages` and
/// delete it again; they stay clear of the conversations the list and search
/// tests read.
///
/// ⚠ `irc_conversation_stats` is seeded ONCE from the rows and is NOT maintained
/// here — production keeps it current with triggers this suite has no copy of.
/// So a test that inserts a line and then asserts on `list_conversations` would
/// be reading a count that deliberately did not move. Assert on `irc_messages`
/// directly, as the send-path tests do.
static FIXTURE: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// A pool onto the seeded fixture, or None when the DB tests are being skipped.
async fn seeded_pool() -> Option<MySqlPool> {
    let pool = test_pool().await?;
    FIXTURE.get_or_init(|| seed(&pool)).await;
    Some(pool)
}

async fn seed(pool: &MySqlPool) {
    // Throwaway DB: start from a clean slate every run.
    for t in [
        "reactions",
        "attachments",
        "messages",
        "conversations",
        "contacts",
        "gchat_reactions",
        "gchat_messages",
        "gchat_conversations",
        "irc_conversation_stats",
        "irc_messages",
        "irc_conversations",
        "sessions",
    ] {
        let _ = sqlx::query(AssertSqlSafe(format!("DROP TABLE IF EXISTS {t}")))
            .execute(pool)
            .await;
    }
    let ddl = [
        "CREATE TABLE conversations (thread_id VARCHAR(80) PRIMARY KEY, type ENUM('dm','group') NOT NULL, name VARCHAR(255) NULL) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE contacts (uuid VARCHAR(64) PRIMARY KEY, phone VARCHAR(32) NULL, profile_name VARCHAR(255) NULL) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE messages (id BIGINT AUTO_INCREMENT PRIMARY KEY, thread_id VARCHAR(80) NOT NULL, sender_uuid VARCHAR(64) NOT NULL, server_ts BIGINT NOT NULL, body TEXT NULL, quote_target_ts BIGINT NULL, is_outgoing TINYINT(1) NOT NULL DEFAULT 0, deleted TINYINT(1) NOT NULL DEFAULT 0, edited TINYINT(1) NOT NULL DEFAULT 0, edit_of_ts BIGINT NULL) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE reactions (id BIGINT AUTO_INCREMENT PRIMARY KEY, thread_id VARCHAR(80) NOT NULL, target_ts BIGINT NOT NULL, author_uuid VARCHAR(64) NOT NULL, emoji VARCHAR(32) NULL, reaction_ts BIGINT NOT NULL, removed TINYINT(1) NOT NULL DEFAULT 0) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE attachments (id BIGINT AUTO_INCREMENT PRIMARY KEY, message_id BIGINT NOT NULL, content_type VARCHAR(255) NULL, file_name VARCHAR(512) NULL, size_bytes BIGINT NULL, stored_path VARCHAR(1024) NULL) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE gchat_conversations (group_id VARCHAR(64) PRIMARY KEY, name VARCHAR(255) NULL, is_dm TINYINT(1) NOT NULL DEFAULT 0) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE gchat_messages (id BIGINT AUTO_INCREMENT PRIMARY KEY, group_id VARCHAR(64) NOT NULL, msg_id VARCHAR(64) NOT NULL, thread_id VARCHAR(64) NULL, sender_id VARCHAR(32) NULL, sender_name VARCHAR(255) NULL, is_self TINYINT(1) NOT NULL DEFAULT 0, ts_us BIGINT NOT NULL, sent_at DATETIME(6) NULL, text TEXT NULL) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE gchat_reactions (id BIGINT AUTO_INCREMENT PRIMARY KEY, message_id BIGINT NOT NULL, emoji VARCHAR(64) NULL, cnt INT NOT NULL DEFAULT 0) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE irc_conversations (id INT AUTO_INCREMENT PRIMARY KEY, network VARCHAR(64) NOT NULL, target VARCHAR(255) NOT NULL, is_channel TINYINT(1) NOT NULL DEFAULT 0, is_status TINYINT(1) NOT NULL DEFAULT 0) DEFAULT CHARSET=utf8mb4",
        // ⚠ `uniq_irc_line` IS NOT DECORATION HERE. It is the archive's dedupe
        // key, and the send path writes a row that the hourly importer will
        // later write again from the same log line — the constraint is the only
        // thing that makes those one row instead of two. A fixture without it
        // lets a dedupe test pass while proving nothing, which is what this
        // table did until the send path needed to rely on it.
        "CREATE TABLE irc_messages (id BIGINT AUTO_INCREMENT PRIMARY KEY, conversation_id INT NOT NULL, source_tag VARCHAR(64) NOT NULL, file_date DATE NOT NULL, line_no INT NOT NULL, sent_at DATETIME NOT NULL, nick VARCHAR(255) NULL, is_self TINYINT(1) NOT NULL DEFAULT 0, kind ENUM('message','action','event','notice') NOT NULL, text TEXT NULL, UNIQUE KEY uniq_irc_line (conversation_id, source_tag, file_date, line_no)) DEFAULT CHARSET=utf8mb4",
        // No trigger here: production maintains this from `signal`'s migrations
        // v11-v14, and this suite seeds it from the rows instead (see `seed`).
        "CREATE TABLE irc_conversation_stats (conversation_id INT NOT NULL PRIMARY KEY, cnt BIGINT NOT NULL DEFAULT 0, last_sent_at DATETIME NULL) DEFAULT CHARSET=utf8mb4",
    ];
    for stmt in ddl {
        sqlx::query(stmt).execute(pool).await.expect("ddl");
    }

    // Signal: a DM (Alice) with 4 messages + reactions, and a group with 1.
    sqlx::query("INSERT INTO conversations (thread_id, type, name) VALUES ('dm:alice','dm','Alice'),('group:g1','group','Grp')").execute(pool).await.unwrap();
    sqlx::query("INSERT INTO contacts (uuid, profile_name) VALUES ('alice','Alice'),('me','Me')")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO messages (thread_id, sender_uuid, server_ts, body, is_outgoing, deleted, edited) VALUES
         ('dm:alice','alice',1000,'hi',0,0,0),
         ('dm:alice','me',2000,'yo',1,0,0),
         ('dm:alice','alice',3000,'edited one',0,0,1),
         ('dm:alice','me',4000,'gone',1,1,0),
         ('group:g1','alice',5000,'grp findme msg',0,0,0)",
    ).execute(pool).await.unwrap();
    // On the ts=2000 message: 👍 from two authors (count 2), 😂 removed (excluded).
    sqlx::query(
        "INSERT INTO reactions (thread_id, target_ts, author_uuid, emoji, reaction_ts, removed) VALUES
         ('dm:alice',2000,'alice','👍',2100,0),
         ('dm:alice',2000,'bob','👍',2200,0),
         ('dm:alice',2000,'carol','😂',2300,1)",
    ).execute(pool).await.unwrap();

    // A thread whose 4 messages ALL share one server_ts (1500) — the case a
    // millisecond-only cursor would split and drop rows from. Ids ascend with
    // insert order, so the id tie-break yields a,b,c,d.
    sqlx::query("INSERT INTO conversations (thread_id, type, name) VALUES ('dm:tie','dm','Tie')")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO messages (thread_id, sender_uuid, server_ts, body, is_outgoing, deleted, edited) VALUES
         ('dm:tie','alice',1500,'tie a',0,0,0),
         ('dm:tie','alice',1500,'tie b',0,0,0),
         ('dm:tie','alice',1500,'tie c',0,0,0),
         ('dm:tie','alice',1500,'tie d',0,0,0)",
    ).execute(pool).await.unwrap();

    // Google Chat: a DM (Bob) with 2 messages + an aggregated reaction, and an
    // empty group (no messages → last_ts None).
    sqlx::query("INSERT INTO gchat_conversations (group_id, name, is_dm) VALUES ('gc1','Bob',1),('gc2','Team',0)").execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO gchat_messages (group_id, msg_id, sender_name, is_self, ts_us, text) VALUES
         ('gc1','m1','Bob',0,6000000,'hello findme'),
         ('gc1','m2','Me',1,7000000,'hey')",
    )
    .execute(pool)
    .await
    .unwrap();
    let m2: i64 =
        sqlx::query_scalar("SELECT id FROM gchat_messages WHERE group_id='gc1' AND msg_id='m2'")
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO gchat_reactions (message_id, emoji, cnt) VALUES (?, '❤️', 3)")
        .bind(m2)
        .execute(pool)
        .await
        .unwrap();

    // Two attachments on the ts=1000 'hi' message: an image with bytes on the
    // PVC (available), and a metadata-only PDF (history import, no bytes).
    let hi: i64 =
        sqlx::query_scalar("SELECT id FROM messages WHERE thread_id='dm:alice' AND server_ts=1000")
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO attachments (message_id, content_type, file_name, size_bytes, stored_path) VALUES
         (?, 'image/jpeg', 'pic.jpg', 1234, '/attachments/pic_jpg'),
         (?, 'application/pdf', 'doc.pdf', 5678, NULL)",
    ).bind(hi).bind(hi).execute(pool).await.unwrap();

    // IRC. Three conversations covering what the origin has that the others do
    // not: a channel, a query, and the status pseudo-conversation irssi files
    // server notices into — named after your own nick, and not a conversation.
    //
    // Every one carries non-speech rows (a join, a notice) because those are the
    // bulk of the real archive: 385,012 notices against 425,748 messages in the
    // tree this was built from. A query that forgets to exclude them looks fine
    // against a fixture that has none.
    sqlx::query(
        "INSERT INTO irc_conversations (network, target, is_channel, is_status) VALUES
         ('net','#chan',1,0),('net','carol',0,0),('net','me',0,1)",
    )
    .execute(pool)
    .await
    .unwrap();
    let chan: i32 = sqlx::query_scalar("SELECT id FROM irc_conversations WHERE target='#chan'")
        .fetch_one(pool)
        .await
        .unwrap();
    let carol: i32 = sqlx::query_scalar("SELECT id FROM irc_conversations WHERE target='carol'")
        .fetch_one(pool)
        .await
        .unwrap();
    let status: i32 = sqlx::query_scalar("SELECT id FROM irc_conversations WHERE target='me'")
        .fetch_one(pool)
        .await
        .unwrap();
    // 2020-01-01 00:00:00Z is 1577836800; each minute after it adds 60. Three of
    // the channel's lines share 00:01 — irssi records `%H:%M` and no seconds, so
    // a whole minute sharing one timestamp is the normal case here, not an edge
    // one, and row id is what actually orders them.
    sqlx::query(
        "INSERT INTO irc_messages (conversation_id, source_tag, file_date, line_no, sent_at, nick, is_self, kind, text) VALUES
         (?,'net','2020-01-01',1,'2020-01-01 00:00:00','alice',0,'event','alice has joined #chan'),
         (?,'net','2020-01-01',2,'2020-01-01 00:01:00','alice',0,'message','first findme'),
         (?,'net','2020-01-01',3,'2020-01-01 00:01:00','me',1,'message','second'),
         (?,'net','2020-01-01',4,'2020-01-01 00:01:00','alice',0,'action','waves'),
         (?,'net','2020-01-01',5,'2020-01-01 00:02:00','irc.example.invalid',0,'notice','findme in a notice')",
    )
    .bind(chan).bind(chan).bind(chan).bind(chan).bind(chan)
    .execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO irc_messages (conversation_id, source_tag, file_date, line_no, sent_at, nick, is_self, kind, text) VALUES
         (?,'net','2020-01-01',1,'2020-01-01 00:03:00','carol',0,'message','hello there')",
    )
    .bind(carol)
    .execute(pool).await.unwrap();
    // The newest IRC rows of all, and neither may ever surface: the status log
    // would otherwise sort to the top of the conversation list.
    //
    // ⚠ The second is a *message*, not a notice, and it is deliberate. In the
    // real archive the status log holds nothing but notices, so excluding it by
    // `is_status` and excluding notices by `kind` look like the same rule and
    // one of them tests as redundant. They are not the same rule: `is_status`
    // says this is not a conversation at all, whatever it contains. Without it,
    // one message here — a nick you once held becoming a target, a note typed at
    // yourself — becomes a search hit for a conversation the list refuses to
    // show, which is a result you cannot open.
    sqlx::query(
        "INSERT INTO irc_messages (conversation_id, source_tag, file_date, line_no, sent_at, nick, is_self, kind, text) VALUES
         (?,'net','2020-01-01',1,'2020-01-01 00:09:00','irc.example.invalid',0,'notice','findme motd'),
         (?,'net','2020-01-01',2,'2020-01-01 00:10:00','me',1,'message','findme note to self')",
    )
    .bind(status).bind(status)
    .execute(pool).await.unwrap();

    // The conversation list reads `irc_conversation_stats` rather than
    // aggregating, so the fixture has to hold it too.
    //
    // ⚠ DERIVED FROM THE ROWS, never hand-written. In production this table is
    // maintained by triggers that live in the `signal` repo — this suite cannot
    // test those, and duplicating their logic here would let the copy drift into
    // agreeing with a query that is wrong. Computing it with the documented
    // backfill statement instead means the fixture asserts the one property that
    // actually matters to this repo: the list agrees with the aggregate.
    sqlx::query(
        "INSERT INTO irc_conversation_stats (conversation_id, cnt, last_sent_at)
         SELECT conversation_id, COUNT(*), MAX(sent_at) FROM irc_messages
          WHERE kind IN ('message', 'action') GROUP BY conversation_id",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn conversations_normalise_and_sort_across_origins() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };

    let convs = archive::list_conversations(&pool).await.unwrap();
    // Newest activity first. The IRC pair leads because its fixture carries real
    // datetimes (2020) while the Signal and Google Chat rows carry bare epoch
    // milliseconds in 1970 — carol last spoke 00:03, #chan 00:01. Their ids are
    // auto-increment, so they are looked up rather than written down here.
    // Then: gc1(7000), group:g1(5000), dm:alice(4000), dm:tie(1500), gc2(None).
    let irc_id = |name: &str| {
        convs
            .iter()
            .find(|c| c.origin == Origin::Irc && c.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no IRC conversation named {name}"))
            .id
            .clone()
    };
    let ids: Vec<_> = convs.iter().map(|c| c.id.clone()).collect();
    assert_eq!(
        ids,
        [
            irc_id("carol"),
            irc_id("#chan"),
            "gc1".to_string(),
            "group:g1".to_string(),
            "dm:alice".to_string(),
            "dm:tie".to_string(),
            "gc2".to_string(),
        ],
        "sort by last_ts desc"
    );

    let by = |id: &str| convs.iter().find(|c| c.id == id).unwrap();
    assert_eq!(
        (
            by("dm:alice").origin,
            by("dm:alice").kind,
            by("dm:alice").message_count,
            by("dm:alice").last_ts
        ),
        (Origin::Signal, ConversationKind::Dm, 4, Some(4000))
    );
    assert_eq!(
        (by("group:g1").kind, by("group:g1").message_count),
        (ConversationKind::Group, 1)
    );
    assert_eq!(
        (
            by("gc1").origin,
            by("gc1").kind,
            by("gc1").message_count,
            by("gc1").last_ts
        ),
        (Origin::Gchat, ConversationKind::Dm, 2, Some(7000))
    );
    assert_eq!(
        (by("gc2").message_count, by("gc2").last_ts),
        (0, None),
        "empty conv: 0 msgs, no last_ts"
    );
}

#[tokio::test]
async fn signal_messages_flags_reactions_and_pagination() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let page = archive::messages_page(&pool, Origin::Signal, "dm:alice", None, 100)
        .await
        .unwrap();
    let ts: Vec<_> = page.messages.iter().map(|m| m.ts).collect();
    assert_eq!(ts, [1000, 2000, 3000, 4000], "ascending");
    assert!(!page.has_more);

    let m2 = &page.messages[1];
    assert!(
        m2.is_outgoing && m2.sender == "Me",
        "contact name + outgoing"
    );
    assert_eq!(m2.reactions.len(), 1, "👍 only (😂 was removed)");
    assert_eq!(
        (m2.reactions[0].emoji.as_str(), m2.reactions[0].count),
        ("👍", 2)
    );
    assert!(page.messages[2].edited, "ts=3000 edited");
    assert!(page.messages[3].deleted, "ts=4000 deleted");

    // Cursor walk with a tiny page size returns every message, in order, once.
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let p = archive::messages_page(&pool, Origin::Signal, "dm:alice", cursor, 2)
            .await
            .unwrap();
        if p.messages.is_empty() {
            break;
        }
        seen.splice(0..0, p.messages.iter().map(|m| m.ts));
        cursor = p.next_cursor.as_deref().and_then(parse_cursor);
        if !p.has_more {
            break;
        }
    }
    assert_eq!(
        seen,
        [1000, 2000, 3000, 4000],
        "paginated walk covers all in order"
    );
}

#[tokio::test]
async fn pagination_never_skips_messages_sharing_a_timestamp() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    // dm:tie has 4 messages all at server_ts=1500. Walk two-at-a-time: a bare-ts
    // cursor would fetch the first page, set the cursor to 1500, then `server_ts <
    // 1500` returns nothing — losing two messages. The (ts,id) cursor recovers all
    // four, in order.
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let p = archive::messages_page(&pool, Origin::Signal, "dm:tie", cursor, 2)
            .await
            .unwrap();
        if p.messages.is_empty() {
            break;
        }
        let bodies = p.messages.iter().map(|m| m.body.clone().unwrap());
        seen.splice(0..0, bodies);
        cursor = p.next_cursor.as_deref().and_then(parse_cursor);
        if !p.has_more {
            break;
        }
    }
    assert_eq!(seen, ["tie a", "tie b", "tie c", "tie d"], "no row skipped");
}

#[tokio::test]
async fn signal_attachments_available_flag_and_blob_lookup() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let page = archive::messages_page(&pool, Origin::Signal, "dm:alice", None, 100)
        .await
        .unwrap();
    let m0 = &page.messages[0]; // ts=1000 'hi'
    assert_eq!(m0.attachments.len(), 2);
    let img = m0
        .attachments
        .iter()
        .find(|a| a.is_image)
        .expect("image attachment");
    assert!(
        img.available
            && img.content_type.as_deref() == Some("image/jpeg")
            && img.file_name.as_deref() == Some("pic.jpg")
    );
    let pdf = m0
        .attachments
        .iter()
        .find(|a| !a.is_image)
        .expect("pdf attachment");
    assert!(!pdf.available, "metadata-only attachment is not available");

    // Other messages have no attachments.
    assert!(page.messages[1].attachments.is_empty());

    // Blob lookup: present for the image (with bytes), absent for the PDF (none).
    let img_id: i64 = img.id.parse().unwrap();
    let pdf_id: i64 = pdf.id.parse().unwrap();
    assert_eq!(
        archive::attachment_blob(&pool, img_id).await.unwrap(),
        Some((
            Some("image/jpeg".to_string()),
            "/attachments/pic_jpg".to_string()
        )),
    );
    assert_eq!(archive::attachment_blob(&pool, pdf_id).await.unwrap(), None);
}

#[tokio::test]
async fn gchat_messages_convert_us_and_self() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let page = archive::messages_page(&pool, Origin::Gchat, "gc1", None, 100)
        .await
        .unwrap();
    let ts: Vec<_> = page.messages.iter().map(|m| m.ts).collect();
    assert_eq!(ts, [6000, 7000], "µs→ms, ascending");
    assert!(!page.messages[0].is_outgoing && page.messages[0].sender == "Bob");
    let hey = &page.messages[1];
    assert!(hey.is_outgoing, "is_self → is_outgoing");
    assert_eq!(
        (hey.reactions[0].emoji.as_str(), hey.reactions[0].count),
        ("❤️", 3)
    );
}

#[tokio::test]
async fn search_spans_origins_excludes_deleted_newest_first() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let hits = archive::search(&pool, "findme", 50).await.unwrap();
    // Three of the fixture's six 'findme' rows. The other three are IRC and must
    // not surface: a server notice in #chan, a notice in the status log, and a
    // *message* in the status log — the newest row of all, and the one that
    // separates the two exclusions. Drop `kind` and the notices appear; drop
    // `is_status` and that message appears, first, in a conversation the list
    // will not show.
    assert_eq!(
        hits.len(),
        3,
        "irc 'first findme' + gchat 'hello findme' + signal 'grp findme msg'"
    );
    assert_eq!(
        hits[0].origin,
        Origin::Irc,
        "newest first — the IRC fixture is 2020, the others are epoch-1970"
    );
    assert_eq!(hits[0].snippet, "first findme");
    assert_eq!(hits[0].conversation_name.as_deref(), Some("#chan"));
    assert_eq!(
        hits[1].origin,
        Origin::Gchat,
        "then gc1 m1 @6000 > group @5000"
    );
    assert_eq!(hits[2].conversation_id, "group:g1");

    // The deleted Signal message 'gone' must never surface.
    assert!(archive::search(&pool, "gone", 50).await.unwrap().is_empty());
}

/// A short page must not be filled by rows that were going to be thrown away.
///
/// ⚠ This is the hazard the fast IRC query shape introduces, and nothing else in
/// the suite would catch it. The scan has to happen *before* the join to
/// `irc_conversations` — joined the other way round the optimizer reads 3.7M
/// rows by index lookup and search takes 32s — so `is_status` moves inside the
/// derived table as a subquery. Leave it outside, as a condition on the join,
/// and it filters rows the `LIMIT` has already spent: the newest `findme` in
/// the whole fixture is the status log's, so a limit of one would return the
/// status row from the scan, drop it in the join, and hand back a page missing
/// the hit it should have contained.
#[tokio::test]
async fn search_applies_the_status_exclusion_before_the_limit() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let hits = archive::search(&pool, "findme", 1).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        (hits[0].origin, hits[0].snippet.as_str()),
        (Origin::Irc, "first findme"),
        "the status log's newer 'findme' must not consume the one slot"
    );
}

/// The status log is left out and only speech is counted.
///
/// Its notice is the newest IRC row in the fixture, so a query that dropped the
/// `is_status` filter would not merely include it — it would put a conversation
/// with yourself, containing nothing you wrote, at the top of the list.
#[tokio::test]
async fn irc_conversations_leave_out_the_status_log_and_count_only_speech() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let convs = archive::list_conversations(&pool).await.unwrap();
    let irc: Vec<_> = convs.iter().filter(|c| c.origin == Origin::Irc).collect();
    let names: Vec<_> = irc.iter().filter_map(|c| c.name.as_deref()).collect();
    assert_eq!(
        names,
        ["carol", "#chan"],
        "the status log is not a conversation"
    );

    let chan = irc
        .iter()
        .find(|c| c.name.as_deref() == Some("#chan"))
        .unwrap();
    assert_eq!(chan.kind, ConversationKind::Group, "a channel is a group");
    assert_eq!(
        chan.message_count, 3,
        "2 messages + 1 action; the join and the notice are not conversation"
    );
    assert_eq!(
        chan.last_ts,
        Some(1_577_836_860_000),
        "last activity is the action at 00:01, not the notice at 00:02"
    );

    let carol = irc
        .iter()
        .find(|c| c.name.as_deref() == Some("carol"))
        .unwrap();
    assert_eq!(carol.kind, ConversationKind::Dm, "a query is a DM");
}

/// A page carries what was said and nothing else, and an action is marked as
/// one. The sender travels in its own field, so the leading star is all that is
/// left to say this was `/me` rather than speech.
#[tokio::test]
async fn irc_page_shows_speech_only_and_marks_actions() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let convs = archive::list_conversations(&pool).await.unwrap();
    let chan = convs
        .iter()
        .find(|c| c.origin == Origin::Irc && c.name.as_deref() == Some("#chan"))
        .unwrap();
    let page = archive::messages_page(&pool, Origin::Irc, &chan.id, None, 50)
        .await
        .unwrap();

    let bodies: Vec<_> = page
        .messages
        .iter()
        .map(|m| m.body.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        bodies,
        ["first findme", "second", "* waves"],
        "join and notice excluded; the action keeps its star"
    );
    assert_eq!(
        page.messages
            .iter()
            .map(|m| m.is_outgoing)
            .collect::<Vec<_>>(),
        [false, true, false],
        "is_self becomes is_outgoing"
    );
    assert_eq!(page.messages[0].sender, "alice");
    assert_eq!(
        page.messages.len() as i64,
        chan.message_count,
        "the list must not promise more messages than the page will show"
    );
}

/// ⚠ Three of the channel's lines share one minute, which is the *normal* case
/// for this origin rather than an edge one: irssi records `%H:%M` and no
/// seconds. Ordering therefore rests entirely on row id, which the importer
/// makes meaningful by walking files in sorted path order.
#[tokio::test]
async fn irc_page_orders_lines_that_share_a_minute() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let convs = archive::list_conversations(&pool).await.unwrap();
    let chan = convs
        .iter()
        .find(|c| c.origin == Origin::Irc && c.name.as_deref() == Some("#chan"))
        .unwrap();

    // One at a time, paging back through a single shared timestamp: the case a
    // ts-only cursor cannot express, because every row's ts is equal.
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = archive::messages_page(&pool, Origin::Irc, &chan.id, cursor, 1)
            .await
            .unwrap();
        let Some(m) = page.messages.first() else {
            break;
        };
        seen.push(m.body.clone().unwrap_or_default());
        assert_eq!(m.ts, 1_577_836_860_000, "all three share 00:01");
        match page.next_cursor.as_deref().and_then(parse_cursor) {
            Some(c) if page.has_more => cursor = Some(c),
            _ => break,
        }
    }
    seen.reverse(); // paged newest→oldest
    assert_eq!(seen, ["first findme", "second", "* waves"], "file order");
}

#[tokio::test]
async fn irc_target_names_the_network_and_flags_the_status_log() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };

    let convs = archive::list_conversations(&pool).await.unwrap();
    let carol = convs
        .iter()
        .find(|c| c.origin == Origin::Irc && c.name.as_deref() == Some("carol"))
        .unwrap();

    let t = archive::irc_target(&pool, &carol.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.network, "net");
    assert_eq!(t.target, "carol");
    assert!(!t.is_status);

    // The status pseudo-conversation is not in list_conversations at all — the
    // reader hides it — so it is looked up by the id the fixture gave it. The
    // sender has to refuse it, and can only do that if this says so.
    let status: i32 = sqlx::query_scalar("SELECT id FROM irc_conversations WHERE target='me'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let t = archive::irc_target(&pool, &status.to_string())
        .await
        .unwrap()
        .unwrap();
    assert!(
        t.is_status,
        "irssi files server notices under your own nick"
    );

    assert!(
        archive::irc_target(&pool, "99999").await.unwrap().is_none(),
        "a conversation that does not exist is None, not an error"
    );
}

/// ⚠ The one that matters: the row the send path writes and the row the hourly
/// importer writes for the same log line must be THE SAME ROW.
///
/// They are written by different programs in different repositories, minutes to
/// an hour apart, and nothing but the unique key makes them one message. Get the
/// key wrong and nothing fails — the conversation just shows what Pippijn said
/// twice.
#[tokio::test]
async fn a_sent_message_and_its_later_import_are_one_row() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };

    let carol: i32 = sqlx::query_scalar("SELECT id FROM irc_conversations WHERE target='carol'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let id = carol.to_string();

    let sent = messages::irc_send::Sent {
        // The tag irssi reported, which is what the importer takes from the log
        // file's path — not the conversation's network, which `--map` may have
        // rewritten.
        tag: "net".to_string(),
        nick: "me".to_string(),
        text: "sent from the phone".to_string(),
        is_action: false,
        logged: Some(messages::irc_send::Logged {
            file_date: "2020-01-02".to_string(),
            line_no: 7,
            line: "00:04 <me> sent from the phone".to_string(),
        }),
    };

    let wrote = messages::irc_send::record_echo(&pool, &id, &sent)
        .await
        .unwrap();
    assert!(wrote, "the echo is written so it can be shown at once");

    // Now the importer, reading the same line out of irssi's log an hour later.
    // This is exactly `insert_irc_line` in the signal repo: INSERT IGNORE on
    // (conversation, source_tag, file_date, line_no).
    let importer = sqlx::query(
        "INSERT IGNORE INTO irc_messages
           (conversation_id, source_tag, file_date, line_no, sent_at, nick, is_self, kind, text)
         VALUES (?, 'net', '2020-01-02', 7, '2020-01-02 00:04:00', 'me', 1, 'message', 'sent from the phone')",
    )
    .bind(&id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        importer.rows_affected(),
        0,
        "the import must find it already present, not add a second copy"
    );

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM irc_messages WHERE conversation_id = ? AND file_date = '2020-01-02'",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "one message, however many times it is written");

    // And the timestamp came from the log line, not from the clock — otherwise
    // the message sorts by when the request happened to be served.
    let at: String = sqlx::query_scalar(
        "SELECT DATE_FORMAT(sent_at, '%Y-%m-%d %H:%i:%s') FROM irc_messages
         WHERE conversation_id = ? AND file_date = '2020-01-02'",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(at, "2020-01-02 00:04:00");

    sqlx::query("DELETE FROM irc_messages WHERE conversation_id = ? AND file_date = '2020-01-02'")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
}

/// A send that irssi could not find in the log is still a send. Recording
/// nothing is right — the importer will pick the line up — and claiming failure
/// would invite sending it twice.
#[tokio::test]
async fn an_unlogged_send_records_nothing_and_is_not_an_error() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };

    let carol: i32 = sqlx::query_scalar("SELECT id FROM irc_conversations WHERE target='carol'")
        .fetch_one(&pool)
        .await
        .unwrap();

    let sent = messages::irc_send::Sent {
        tag: "net".to_string(),
        nick: "me".to_string(),
        text: "gone, but not seen".to_string(),
        is_action: false,
        logged: None,
    };
    assert!(
        !messages::irc_send::record_echo(&pool, &carol.to_string(), &sent)
            .await
            .unwrap()
    );
}

/// irssi records `%H:%M` and nothing finer, and the date comes from the log
/// file's path — so `sent_at` has to be read off the line, not off the clock.
/// Reading the clock would disagree with the importer for the same line whenever
/// a send straddled a minute, and `sent_at` is what the reader orders by.
#[tokio::test]
async fn the_echo_takes_its_timestamp_from_the_log_line() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };

    let carol: i32 = sqlx::query_scalar("SELECT id FROM irc_conversations WHERE target='carol'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let id = carol.to_string();

    // ⚠ `< nick>` — the space is the CHANNEL MODE, not part of the name. It is
    // how an unopped speaker appears in a channel, and it must not throw the
    // timestamp off.
    let sent = messages::irc_send::Sent {
        tag: "net".to_string(),
        nick: "me".to_string(),
        text: "from a channel".to_string(),
        is_action: false,
        logged: Some(messages::irc_send::Logged {
            file_date: "2020-01-03".to_string(),
            line_no: 2,
            line: "09:05 < me> from a channel".to_string(),
        }),
    };
    assert!(
        messages::irc_send::record_echo(&pool, &id, &sent)
            .await
            .unwrap()
    );
    let at: String = sqlx::query_scalar(
        "SELECT DATE_FORMAT(sent_at, '%Y-%m-%d %H:%i:%s') FROM irc_messages
         WHERE conversation_id = ? AND file_date = '2020-01-03'",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(at, "2020-01-03 09:05:00");

    // A line that is not shaped like a log line records nothing rather than
    // guessing a time: a guessed one puts the message in the wrong place in the
    // conversation, where leaving it to the import puts it in the right place,
    // late.
    for line in ["--- Log opened", "", "1:01 x", "aa:bb x"] {
        let odd = messages::irc_send::Sent {
            tag: "net".to_string(),
            nick: "me".to_string(),
            text: "unplaceable".to_string(),
            is_action: false,
            logged: Some(messages::irc_send::Logged {
                file_date: "2020-01-04".to_string(),
                line_no: 1,
                line: line.to_string(),
            }),
        };
        assert!(
            !messages::irc_send::record_echo(&pool, &id, &odd)
                .await
                .unwrap(),
            "no row for a line with no timestamp: {line:?}"
        );
    }

    sqlx::query("DELETE FROM irc_messages WHERE conversation_id = ? AND file_date = '2020-01-03'")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
}

// ---- what a leading slash means (no DB) -------------------------------------

/// ⚠ **`/me` WENT OUT AS FOUR LITERAL CHARACTERS**, measured 2026-08-14: typed
/// into the composer it reached `#linux` as the text `/me …` rather than as an
/// action. The send path hands the composer's words to irssi as DATA that never
/// reaches a command parser — that is what makes `/exec` and an embedded newline
/// harmless from a web request — and the price was that the one "command" which
/// is really content went out verbatim.
#[test]
fn a_leading_slash_means_an_action_an_escape_or_nothing() {
    use messages::irc_send::parse_slash;

    assert_eq!(parse_slash("/me waves"), ("waves", true));
    assert_eq!(parse_slash("/me  padded "), (" padded ", true));

    // IRC's own escape, so a literal leading slash is still sayable.
    assert_eq!(parse_slash("//me waves"), ("/me waves", false));
    assert_eq!(parse_slash("//quit"), ("/quit", false));

    // ⚠ NOT refused, and this is the case that matters in #linux: a message
    // that happens to start with a path is ordinary text, and rejecting every
    // unknown slash word would break it to catch a typo that is harmless
    // anyway — none of these reach a parser.
    assert_eq!(
        parse_slash("/usr/bin/foo is broken"),
        ("/usr/bin/foo is broken", false)
    );
    assert_eq!(parse_slash("/quit"), ("/quit", false));

    // A bare `/me` with nothing after it is not an action with an empty body —
    // the plugin would refuse empty text and the send would fail for a reason
    // the person could not act on.
    assert_eq!(parse_slash("/me"), ("/me", false));
    assert_eq!(parse_slash("/me   "), ("/me   ", false));

    assert_eq!(parse_slash("ordinary words"), ("ordinary words", false));
}
