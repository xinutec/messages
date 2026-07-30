//! Tests for the archive query/normalisation layer.
//!
//! Pure unit tests (timestamp/kind/LIKE-escape) always run. The end-to-end DB
//! tests seed a known fixture into a MariaDB and assert the real queries —
//! ordering, the `before` pagination cursor, Signal reaction aggregation, edit/
//! delete flags, the µs→ms conversion, and cross-origin search. They run when
//! `MESSAGES_TEST_DATABASE_URL` points at a *throwaway* database (the test
//! drops+recreates the archive tables), and are skipped otherwise. CI sets it
//! from a MariaDB service; locally `scripts/with-test-db.sh` starts one and
//! `scripts/verify.sh` runs the suite through it, so the pre-commit gate covers
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
/// One shared fixture is sound *because* every test here only reads — this app
/// is a read-only viewer and none of the queries under test write. A test that
/// ever mutates needs its own database, not a slot in this one.
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
}

#[tokio::test]
async fn conversations_normalise_and_sort_across_origins() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };

    let convs = archive::list_conversations(&pool).await.unwrap();
    // Newest activity first: gc1(7000), group:g1(5000), dm:alice(4000),
    // dm:tie(1500), gc2(None).
    let ids: Vec<_> = convs.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        ["gc1", "group:g1", "dm:alice", "dm:tie", "gc2"],
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
    assert_eq!(
        hits.len(),
        2,
        "gchat 'hello findme' + signal 'grp findme msg'"
    );
    assert_eq!(
        hits[0].origin,
        Origin::Gchat,
        "newest first (gc1 m1 @6000 > group @5000)"
    );
    assert_eq!(hits[1].conversation_id, "group:g1");

    // The deleted Signal message 'gone' must never surface.
    assert!(archive::search(&pool, "gone", 50).await.unwrap().is_empty());
}
