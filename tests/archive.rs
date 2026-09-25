//! Tests for the archive query/normalisation layer.
//!
//! Pure units always run. The database tests seed a fixture into
//! `MESSAGES_TEST_DATABASE_URL`, which must be a throwaway database: the archive
//! tables are dropped and recreated. Skipped when it is unset; CI and the gate
//! supply one. Never point it at the real signal database.

use messages::archive::{
    self, ConversationKind, DeliveryState, EXCERPT_CHARS, MessageKind, Origin, PageDir, call_text,
    encode_cursor, escape_like, excerpt, kind_from_is_dm, parse_cursor, us_to_ms,
};

// ---- pure units (no DB) -----------------------------------------------------

#[test]
fn us_to_ms_truncates_to_millis() {
    assert_eq!(us_to_ms(7_000_000), 7000);
    assert_eq!(us_to_ms(1_584_389_732_190_514), 1_584_389_732_190);
}

/// An `Action` renders as `* Dana <body>`, so the body is a verb phrase. An
/// unanswered call has no duration.
#[test]
fn a_call_reads_as_something_its_sender_did() {
    assert_eq!(
        call_text(Some(3273), Some("hangup"), true).as_deref(),
        Some("made a 55-minute video call")
    );
    assert_eq!(
        call_text(Some(42), Some("hangup"), false).as_deref(),
        Some("made a 42-second call")
    );
    assert_eq!(
        call_text(Some(7_200), Some("hangup"), false).as_deref(),
        Some("made a 2-hour call")
    );
    assert_eq!(
        call_text(None, Some("missed"), false).as_deref(),
        Some("made a call that went unanswered"),
        "no duration is the record of nobody answering, not a call of no length"
    );
    assert_eq!(
        call_text(None, Some("elsewhere"), false).as_deref(),
        Some("made a call (elsewhere)")
    );
    assert_eq!(call_text(None, None, false), None);
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
    assert_eq!(
        ConversationKind::parse("channel"),
        Some(ConversationKind::Channel)
    );
    // Anything else means the schema moved. ENUM values are case-sensitive here.
    assert_eq!(ConversationKind::parse("DM"), None);
    assert_eq!(ConversationKind::parse("broadcast"), None);
    assert_eq!(ConversationKind::parse(""), None);
}

/// The wire spelling is the frontend's contract: the generated TS has string
/// unions.
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
    assert_eq!(
        serde_json::to_string(&MessageKind::Message).unwrap(),
        r#""message""#
    );
    assert_eq!(
        serde_json::to_string(&MessageKind::Action).unwrap(),
        r#""action""#
    );
}

#[test]
fn message_kind_parses_only_the_two_the_queries_admit() {
    assert_eq!(MessageKind::parse("message"), Some(MessageKind::Message));
    assert_eq!(MessageKind::parse("action"), Some(MessageKind::Action));
    // The queries filter these two out.
    assert_eq!(MessageKind::parse("event"), None);
    assert_eq!(MessageKind::parse("notice"), None);
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
    // Malformed: `None`, and the API starts from the newest page.
    assert_eq!(parse_cursor("nope"), None);
    assert_eq!(parse_cursor("123_"), None);
    assert_eq!(parse_cursor("_9"), None);
    assert_eq!(parse_cursor(""), None);
}

#[test]
fn an_excerpt_is_one_line_and_never_splits_a_character() {
    assert_eq!(excerpt(None), None);
    assert_eq!(excerpt(Some("")), None);
    assert_eq!(excerpt(Some("   \n  ")), None);
    assert_eq!(excerpt(Some("hoi")), Some("hoi".to_string()));

    // One line: the body's own breaks are folded.
    assert_eq!(
        excerpt(Some("two\nlines\there")),
        Some("two lines here".to_string())
    );

    // Exactly at the bound: no ellipsis.
    let exact = "a".repeat(EXCERPT_CHARS);
    assert_eq!(excerpt(Some(&exact)), Some(exact.clone()));
    let over = "a".repeat(EXCERPT_CHARS + 1);
    assert_eq!(excerpt(Some(&over)), Some(format!("{exact}…")));

    // By character: each of these is multi-byte, so a byte cut would panic.
    let emoji = "🐉".repeat(EXCERPT_CHARS + 10);
    let cut = excerpt(Some(&emoji)).unwrap();
    assert_eq!(cut.chars().count(), EXCERPT_CHARS + 1, "120 + the ellipsis");
    assert!(cut.starts_with("🐉🐉"));
    let cyrillic = "я".repeat(EXCERPT_CHARS + 10);
    assert_eq!(
        excerpt(Some(&cyrillic)).unwrap().chars().count(),
        EXCERPT_CHARS + 1
    );
}

// ---- end-to-end against a real MariaDB --------------------------------------

use sqlx::mysql::MySqlPoolOptions;
use sqlx::{AssertSqlSafe, MySqlPool};

use messages::config::Config;
use messages::irc_send::IrcSender;
use messages::state::AppState;

async fn test_pool() -> Option<MySqlPool> {
    let url = std::env::var("MESSAGES_TEST_DATABASE_URL").ok()?;
    let pool = MySqlPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect to MESSAGES_TEST_DATABASE_URL");
    Some(pool)
}

/// The fixture is seeded once per process: `seed` drops and recreates the
/// tables, so per-test seeding races under parallel tests.
///
/// A test that writes must touch only rows no other test asserts on.
/// `irc_conversation_stats` is seeded once and not maintained here (production
/// uses triggers), so assert on `irc_messages` after inserting a line.
static FIXTURE: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// A pool onto the seeded fixture, or None when the DB tests are being skipped.
async fn seeded_pool() -> Option<MySqlPool> {
    let pool = test_pool().await?;
    FIXTURE.get_or_init(|| seed(&pool)).await;
    Some(pool)
}

async fn seed(pool: &MySqlPool) {
    for t in [
        "reactions",
        "signal_receipts",
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
        "telegram_media",
        "telegram_reactions",
        "telegram_message_edits",
        "telegram_messages",
        "telegram_message_entities",
        "telegram_read_marks",
        "telegram_conversations",
        "signal_text_styles",
        "signal_link_previews",
        "sessions",
    ] {
        let _ = sqlx::query(AssertSqlSafe(format!("DROP TABLE IF EXISTS {t}")))
            .execute(pool)
            .await;
    }
    let ddl = [
        "CREATE TABLE conversations (thread_id VARCHAR(80) PRIMARY KEY, type ENUM('dm','group') NOT NULL, name VARCHAR(255) NULL, updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE contacts (uuid VARCHAR(64) PRIMARY KEY, phone VARCHAR(32) NULL, display_name VARCHAR(255) NULL, updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE signal_receipts (id BIGINT AUTO_INCREMENT PRIMARY KEY, target_ts BIGINT NOT NULL, author_uuid VARCHAR(64) NOT NULL, kind ENUM('delivery','read','viewed') NOT NULL, when_ts BIGINT NOT NULL, observed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, UNIQUE KEY uniq_signal_receipt (target_ts, author_uuid, kind)) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE messages (id BIGINT AUTO_INCREMENT PRIMARY KEY, thread_id VARCHAR(80) NOT NULL, sender_uuid VARCHAR(64) NOT NULL, server_ts BIGINT NOT NULL, body TEXT NULL, quote_target_ts BIGINT NULL, is_outgoing TINYINT(1) NOT NULL DEFAULT 0, deleted TINYINT(1) NOT NULL DEFAULT 0, edited TINYINT(1) NOT NULL DEFAULT 0, edit_of_ts BIGINT NULL, created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, deleted_at TIMESTAMP NULL, expires_in_seconds INT NULL, server_delivered_ts BIGINT NULL, server_received_ts BIGINT NULL, quote_author_uuid VARCHAR(64) NULL, quote_text TEXT NULL) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE signal_text_styles (id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, message_id BIGINT NOT NULL, style VARCHAR(16) NOT NULL, start_utf16 INT NOT NULL, length_utf16 INT NOT NULL, UNIQUE KEY uniq_signal_text_style (message_id, style, start_utf16, length_utf16)) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE signal_link_previews (id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, message_id BIGINT NOT NULL, position INT NOT NULL, url TEXT NOT NULL, title TEXT NULL, description TEXT NULL, UNIQUE KEY uniq_signal_link_preview (message_id, position)) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE reactions (id BIGINT AUTO_INCREMENT PRIMARY KEY, thread_id VARCHAR(80) NOT NULL, target_ts BIGINT NOT NULL, author_uuid VARCHAR(64) NOT NULL, emoji VARCHAR(32) NULL, reaction_ts BIGINT NOT NULL, removed TINYINT(1) NOT NULL DEFAULT 0) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE attachments (id BIGINT AUTO_INCREMENT PRIMARY KEY, message_id BIGINT NOT NULL, content_type VARCHAR(255) NULL, file_name VARCHAR(512) NULL, size_bytes BIGINT NULL, stored_path VARCHAR(1024) NULL) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE gchat_conversations (group_id VARCHAR(64) PRIMARY KEY, name VARCHAR(255) NULL, is_dm TINYINT(1) NOT NULL DEFAULT 0) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE gchat_messages (id BIGINT AUTO_INCREMENT PRIMARY KEY, group_id VARCHAR(64) NOT NULL, msg_id VARCHAR(64) NOT NULL, thread_id VARCHAR(64) NULL, reply_to_msg_id VARCHAR(64) NULL, sender_id VARCHAR(32) NULL, sender_name VARCHAR(255) NULL, is_self TINYINT(1) NOT NULL DEFAULT 0, ts_us BIGINT NOT NULL, sent_at DATETIME(6) NULL, text TEXT NULL) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE gchat_reactions (id BIGINT AUTO_INCREMENT PRIMARY KEY, message_id BIGINT NOT NULL, emoji VARCHAR(64) NULL, cnt INT NOT NULL DEFAULT 0) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE irc_conversations (id INT AUTO_INCREMENT PRIMARY KEY, network VARCHAR(64) NOT NULL, target VARCHAR(255) NOT NULL, is_channel TINYINT(1) NOT NULL DEFAULT 0, is_status TINYINT(1) NOT NULL DEFAULT 0, updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP) DEFAULT CHARSET=utf8mb4",
        // `uniq_irc_line` is the archive's dedupe key, which the send-path
        // tests rely on.
        "CREATE TABLE irc_messages (id BIGINT AUTO_INCREMENT PRIMARY KEY, conversation_id INT NOT NULL, source_tag VARCHAR(64) NOT NULL, file_date DATE NOT NULL, line_no INT NOT NULL, sent_at DATETIME NOT NULL, nick VARCHAR(255) NULL, is_self TINYINT(1) NOT NULL DEFAULT 0, kind ENUM('message','action','event','notice') NOT NULL, text TEXT NULL, UNIQUE KEY uniq_irc_line (conversation_id, source_tag, file_date, line_no), created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP) DEFAULT CHARSET=utf8mb4",
        // No trigger: `seed` computes this from the rows.
        "CREATE TABLE irc_conversation_stats (conversation_id INT NOT NULL PRIMARY KEY, cnt BIGINT NOT NULL DEFAULT 0, last_sent_at DATETIME NULL, updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP) DEFAULT CHARSET=utf8mb4",
        // A copy of the archiver's tables, and dev-lint reads these
        // CREATE TABLEs to know which tables `src/` may name, so keep them in
        // step with archiver/src/db.rs.
        "CREATE TABLE telegram_conversations (id BIGINT PRIMARY KEY, kind ENUM('dm','group','channel') NOT NULL, name VARCHAR(255) NULL, username VARCHAR(255) NULL, updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE telegram_messages (id BIGINT AUTO_INCREMENT PRIMARY KEY, conversation_id BIGINT NOT NULL, msg_id INT NOT NULL, sent_at BIGINT NOT NULL, sender_id BIGINT NULL, sender_name VARCHAR(255) NULL, is_outgoing TINYINT(1) NOT NULL DEFAULT 0, kind ENUM('message','service') NOT NULL DEFAULT 'message', text TEXT NULL, media_kind VARCHAR(32) NULL, media_size BIGINT NULL, media_mime VARCHAR(128) NULL, edited_at BIGINT NULL, reply_to_msg_id INT NULL, fwd_from_name VARCHAR(255) NULL, edit_hidden TINYINT(1) NULL, deleted TINYINT(1) NOT NULL DEFAULT 0, deleted_at TIMESTAMP NULL, UNIQUE KEY uniq_tg_msg (conversation_id, msg_id), created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, fwd_channel_post INT NULL, fwd_date BIGINT NULL, fwd_from_id BIGINT NULL, grouped_id BIGINT NULL, reply_quote TEXT NULL, reply_to_peer_id BIGINT NULL, service_action VARCHAR(64) NULL, ttl_period INT NULL, via_bot_id BIGINT NULL) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE telegram_message_edits (id BIGINT AUTO_INCREMENT PRIMARY KEY, conversation_id BIGINT NOT NULL, msg_id INT NOT NULL, was_edited_at BIGINT NULL, text TEXT NULL, UNIQUE KEY uniq_tg_edit (conversation_id, msg_id, was_edited_at), recorded_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE telegram_media (conversation_id BIGINT NOT NULL, msg_id INT NOT NULL, state ENUM('offered','wanted','stored','failed') NOT NULL, stored_name VARCHAR(255) NULL, content_type VARCHAR(128) NULL, note VARCHAR(255) NULL, requested_at TIMESTAMP NULL, stored_at TIMESTAMP NULL, PRIMARY KEY (conversation_id, msg_id), updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE telegram_reactions (id BIGINT AUTO_INCREMENT PRIMARY KEY, conversation_id BIGINT NOT NULL, msg_id INT NOT NULL, emoji VARCHAR(32) NULL, custom_emoji_id BIGINT NULL, cnt INT NOT NULL DEFAULT 0, chosen TINYINT(1) NOT NULL DEFAULT 0, removed_at TIMESTAMP NULL, reaction_key VARCHAR(64) GENERATED ALWAYS AS (COALESCE(emoji, CONCAT('custom:', custom_emoji_id), '')) STORED, updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP) DEFAULT CHARSET=utf8mb4",
        // Offsets in UTF-16 code units, as the archive stores them.
        "CREATE TABLE telegram_message_entities (id BIGINT AUTO_INCREMENT PRIMARY KEY, conversation_id BIGINT NOT NULL, msg_id INT NOT NULL, kind VARCHAR(32) NOT NULL, offset_utf16 INT NOT NULL, length_utf16 INT NOT NULL, url TEXT NULL, user_id BIGINT NULL, language VARCHAR(32) NULL, document_id BIGINT NULL, removed_at TIMESTAMP NULL) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE telegram_read_marks (id BIGINT AUTO_INCREMENT PRIMARY KEY, conversation_id BIGINT NOT NULL, direction ENUM('inbox','outbox') NOT NULL, max_id INT NOT NULL, observed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, UNIQUE KEY uniq_tg_read (conversation_id, direction, max_id)) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE gchat_attachments (id BIGINT AUTO_INCREMENT PRIMARY KEY, message_id BIGINT NOT NULL, name VARCHAR(255) NULL, mime VARCHAR(128) NULL, width INT NULL, height INT NULL, uuid VARCHAR(64) NULL, token TEXT NULL, hash1 VARCHAR(128) NULL, hash2 VARCHAR(128) NULL, stored_path VARCHAR(255) NULL) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE gchat_reaction_authors (id BIGINT AUTO_INCREMENT PRIMARY KEY, message_id BIGINT NOT NULL, emoji VARCHAR(64) NOT NULL, reactor_id VARCHAR(32) NOT NULL, UNIQUE KEY uniq_gchat_reactor (message_id, emoji, reactor_id)) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE telegram_reaction_authors (id BIGINT AUTO_INCREMENT PRIMARY KEY, conversation_id BIGINT NOT NULL, msg_id INT NOT NULL, peer_id BIGINT NOT NULL, emoji VARCHAR(32) NULL, custom_emoji_id BIGINT NULL, reacted_at BIGINT NOT NULL, removed_at TIMESTAMP NULL, reaction_key VARCHAR(64) GENERATED ALWAYS AS (COALESCE(emoji, CONCAT('custom:', custom_emoji_id), '')) STORED) DEFAULT CHARSET=utf8mb4",
        "CREATE TABLE telegram_calls (id BIGINT AUTO_INCREMENT PRIMARY KEY, conversation_id BIGINT NOT NULL, msg_id INT NOT NULL, call_id BIGINT NULL, duration_s INT NULL, reason VARCHAR(32) NULL, video TINYINT(1) NOT NULL DEFAULT 0, UNIQUE KEY uniq_tg_call (conversation_id, msg_id)) DEFAULT CHARSET=utf8mb4",
    ];
    for stmt in ddl {
        sqlx::query(stmt).execute(pool).await.expect("ddl");
    }

    // Signal: a DM (Alice) with 4 messages + reactions, and a group with 1.
    sqlx::query("INSERT INTO conversations (thread_id, type, name) VALUES ('dm:alice','dm','Alice'),('group:g1','group','Grp')").execute(pool).await.unwrap();
    sqlx::query("INSERT INTO contacts (uuid, display_name) VALUES ('alice','Alice'),('me','Me')")
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
    // Quotes, by UPDATE so the thread keeps its four messages: one resolves, one
    // targets a deleted message, one a timestamp the archive does not hold.
    sqlx::query(
        "UPDATE messages SET quote_target_ts = CASE server_ts
             WHEN 2000 THEN 1000
             WHEN 3000 THEN 4000
             WHEN 1000 THEN 999
         END
         WHERE thread_id = 'dm:alice' AND server_ts IN (1000, 2000, 3000)",
    )
    .execute(pool)
    .await
    .unwrap();

    // On the ts=2000 message: 👍 from two authors (count 2), 😂 removed (excluded).
    sqlx::query(
        "INSERT INTO reactions (thread_id, target_ts, author_uuid, emoji, reaction_ts, removed) VALUES
         ('dm:alice',2000,'alice','👍',2100,0),
         ('dm:alice',2000,'bob','👍',2200,0),
         ('dm:alice',2000,'carol','😂',2300,1)",
    ).execute(pool).await.unwrap();

    // A thread whose 4 messages share server_ts 1500; the id orders them.
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

    // A thread for receipts. `observed_at` is 1970-01-01 00:00:01, so capture
    // began at 1000ms, and the message at 900 predates it.
    sqlx::query(
        "INSERT INTO conversations (thread_id, type, name) VALUES ('dm:receipts','dm','Receipts')",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO messages (thread_id, sender_uuid, server_ts, body, is_outgoing, deleted, edited) VALUES
         ('dm:receipts','me',900,'before we listened',1,0,0),
         ('dm:receipts','me',1200,'read by her',1,0,0),
         ('dm:receipts','alice',1250,'her words',0,0,0),
         ('dm:receipts','me',1300,'delivered only',1,0,0),
         ('dm:receipts','me',1400,'nothing back',1,0,0)",
    ).execute(pool).await.unwrap();
    // Two traps: `(1200,'me','read')` is my linked device reading my own
    // message, and `(1250,'me','read')` a read of a message Alice sent.
    sqlx::query(
        "INSERT INTO signal_receipts (target_ts, author_uuid, kind, when_ts, observed_at) VALUES
         (1200,'alice','delivery',1210,FROM_UNIXTIME(1)),
         (1200,'alice','read',1220,FROM_UNIXTIME(1)),
         (1200,'me','read',1230,FROM_UNIXTIME(1)),
         (1250,'me','read',1260,FROM_UNIXTIME(1)),
         (1300,'alice','delivery',1310,FROM_UNIXTIME(1))",
    )
    .execute(pool)
    .await
    .unwrap();

    // Google Chat: a DM (Bob) with messages and a reaction, and an empty group.
    sqlx::query("INSERT INTO gchat_conversations (group_id, name, is_dm) VALUES ('gc1','Bob',1),('gc2','Team',0)").execute(pool).await.unwrap();
    sqlx::query(
        // `sender_id` lets a reactor resolve to a name; `g-carol` has never
        // spoken, so falls back to the id. `m2` quote-replies `m1`; `m3` replies
        // to a message the archive does not hold.
        "INSERT INTO gchat_messages (group_id, msg_id, reply_to_msg_id, sender_id, sender_name, is_self, ts_us, text) VALUES
         ('gc1','m1',NULL,'g-bob','Bob',0,6000000,'hello findme'),
         ('gc1','m2','m1','g-me','Me',1,7000000,'hey'),
         ('gc1','m3','gone','g-bob','Bob',0,8000000,'answering something missing')",
    )
    .execute(pool)
    .await
    .unwrap();
    let m2: i64 =
        sqlx::query_scalar("SELECT id FROM gchat_messages WHERE group_id='gc1' AND msg_id='m2'")
            .fetch_one(pool)
            .await
            .unwrap();
    // One picture whose bytes we hold and one we only know about.
    sqlx::query(
        "INSERT INTO gchat_attachments (message_id, name, mime, width, height, uuid, stored_path)
         VALUES (?, 'holiday.jpg', 'image/jpeg', 800, 600, 'u-1', 'abc123'),
                (?, 'lost.webp', 'image/webp', 1080, 745, 'u-2', NULL)",
    )
    .bind(m2)
    .bind(m2)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO gchat_reactions (message_id, emoji, cnt) VALUES (?, '❤️', 3)")
        .bind(m2)
        .execute(pool)
        .await
        .unwrap();
    // Two of three reactors are named, as when the second capture is partial.
    sqlx::query(
        "INSERT INTO gchat_reaction_authors (message_id, emoji, reactor_id) VALUES
         (?, '❤️', 'g-bob'), (?, '❤️', 'g-carol')",
    )
    .bind(m2)
    .bind(m2)
    .execute(pool)
    .await
    .unwrap();

    // Two attachments on the ts=1000 message: an image with bytes, and a
    // metadata-only PDF.
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

    // IRC: a channel, a query, and irssi's status window. Each carries
    // non-speech rows (a join, a notice) the queries must exclude.
    sqlx::query(
        // The last two are one target on two networks.
        "INSERT INTO irc_conversations (network, target, is_channel, is_status) VALUES
         ('net','#chan',1,0),('net','carol',0,0),('net','me',0,1),
         ('xinutec','s_20',0,0),('euirc','s_20',0,0)",
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
    // 2020-01-01 00:00:00Z is 1577836800. Three of the channel's lines share
    // 00:01; the id orders them.
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
    // The `s_20` pair, in 2019, at distinct times.
    for (net, day) in [("xinutec", "2019-01-02"), ("euirc", "2019-01-01")] {
        let id: i32 = sqlx::query_scalar(
            "SELECT id FROM irc_conversations WHERE network=? AND target='s_20'",
        )
        .bind(net)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO irc_messages (conversation_id, source_tag, file_date, line_no, sent_at, nick, is_self, kind, text)
             VALUES (?,?,?,1,?,'s_20',0,'message','same target, other network')",
        )
        .bind(id).bind(net).bind(day).bind(format!("{day} 12:00:00"))
        .execute(pool).await.unwrap();
    }
    // The newest IRC rows, which must never surface. The second is a message:
    // `is_status` excludes the window whatever it holds, separately from the
    // `kind` filter.
    sqlx::query(
        "INSERT INTO irc_messages (conversation_id, source_tag, file_date, line_no, sent_at, nick, is_self, kind, text) VALUES
         (?,'net','2020-01-01',1,'2020-01-01 00:09:00','irc.example.invalid',0,'notice','findme motd'),
         (?,'net','2020-01-01',2,'2020-01-01 00:10:00','me',1,'message','findme note to self')",
    )
    .bind(status).bind(status)
    .execute(pool).await.unwrap();

    // Computed from the rows with the archiver's backfill statement, never written by
    // hand, so the list is checked against the aggregate.
    sqlx::query(
        "INSERT INTO irc_conversation_stats (conversation_id, cnt, last_sent_at)
         SELECT conversation_id, COUNT(*), MAX(sent_at) FROM irc_messages
          WHERE kind IN ('message', 'action') GROUP BY conversation_id",
    )
    .execute(pool)
    .await
    .unwrap();

    // Telegram: a DM and a channel, with a service event, a message edited
    // twice, a retracted one, and two sharing a second. Ids are written in their
    // folded form rather than computed.
    sqlx::query(
        "INSERT INTO telegram_conversations (id, kind, name, username) VALUES
            (4242, 'dm', 'Tessa', 'tessa'),
            (-1000000000055, 'channel', 'Announcements', NULL),
            (-77, 'group', 'Klaverjas', NULL)",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO telegram_messages
            (conversation_id, msg_id, sent_at, sender_id, sender_name, is_outgoing,
             kind, text, edited_at, deleted) VALUES
            (4242, 10, 1700000000, 4242, 'Tessa', 0, 'message', 'hoi', NULL, 0),
            (4242, 11, 1700000060, 777, 'Me', 1, 'message', 'ook hoi', NULL, 0),
            (4242, 12, 1700000060, 4242, 'Tessa', 0, 'message', 'same second', NULL, 0),
            (4242, 13, 1700000120, 4242, 'Tessa', 0, 'message', 'third go', 1700000500, 0),
            (4242, 14, 1700000180, 4242, 'Tessa', 0, 'message', 'forget it', NULL, 1),
            (4242, 15, 1700000240, 4242, 'Tessa', 0, 'service', 'changed the photo', NULL, 0),
            (4242, 16, 1700000260, 777, 'Me', 1, 'message', 'telegram touched this', 1700000600, 0),
            (-1000000000055, 3, 1700000300, NULL, NULL, 0, 'message', 'an announcement', NULL, 0)",
    )
    .execute(pool)
    .await
    .unwrap();
    // An album: msgs 12 and 13 were sent as one set. The id is above 2^53.
    sqlx::query(
        "UPDATE telegram_messages SET grouped_id = 7777777777777777777
          WHERE conversation_id = 4242 AND msg_id IN (12, 13)",
    )
    .execute(pool)
    .await
    .unwrap();
    // Formatting on msg 11, one run retracted.
    sqlx::query(
        "INSERT INTO telegram_message_entities
            (conversation_id, msg_id, kind, offset_utf16, length_utf16, url, removed_at) VALUES
            (4242, 11, 'bold', 0, 3, NULL, NULL),
            (4242, 11, 'textUrl', 4, 3, 'https://example.invalid/a', NULL),
            (4242, 11, 'italic', 0, 3, NULL, CURRENT_TIMESTAMP)",
    )
    .execute(pool)
    .await
    .unwrap();
    // Read marks: they have read up to msg 11. The inbox mark is further along
    // and must not be mistaken for the outbox one.
    sqlx::query(
        "INSERT INTO telegram_read_marks (conversation_id, direction, max_id) VALUES
            (4242, 'outbox', 11),
            (4242, 'inbox', 16)",
    )
    .execute(pool)
    .await
    .unwrap();

    // Replies, by UPDATE: one resolves, one targets a deleted message, one a
    // service event, one an id the archive does not hold.
    sqlx::query(
        "UPDATE telegram_messages SET reply_to_msg_id = CASE msg_id
             WHEN 11 THEN 10
             WHEN 12 THEN 14
             WHEN 13 THEN 15
             WHEN 16 THEN 999
         END
         WHERE conversation_id = 4242 AND msg_id IN (11, 12, 13, 16)",
    )
    .execute(pool)
    .await
    .unwrap();

    // The size lives on the message, as in the archive.
    sqlx::query(
        "UPDATE telegram_messages SET media_kind = 'photo', media_size = 204800, media_mime = 'image/jpeg'
          WHERE conversation_id = 4242 AND msg_id = 14",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE telegram_messages SET media_kind = 'video', media_size = 1610612736, media_mime = 'video/mp4'
          WHERE conversation_id = 4242 AND msg_id = 13",
    )
    .execute(pool)
    .await
    .unwrap();

    // Media: a held photo and an offered video.
    sqlx::query(
        "INSERT INTO telegram_media (conversation_id, msg_id, state, stored_name, content_type) VALUES
            (4242, 14, 'stored', '4242_14', 'image/jpeg'),
            (4242, 13, 'offered', NULL, 'video/mp4')",
    )
    .execute(pool)
    .await
    .unwrap();

    // msg 16 has an edit date and `edit_hide`.
    sqlx::query(
        "UPDATE telegram_messages SET edit_hidden = 1 WHERE conversation_id = 4242 AND msg_id = 16",
    )
    .execute(pool)
    .await
    .unwrap();

    // Two superseded versions of msg 13; the original has no edit date.
    sqlx::query(
        "INSERT INTO telegram_message_edits (conversation_id, msg_id, was_edited_at, text) VALUES
            (4242, 13, NULL, 'first go'),
            (4242, 13, 1700000400, 'second go')",
    )
    .execute(pool)
    .await
    .unwrap();
    // A drawable reaction and a custom emoji.
    sqlx::query(
        "INSERT INTO telegram_reactions (conversation_id, msg_id, emoji, custom_emoji_id, cnt, chosen, removed_at) VALUES
            (4242, 10, '👍', NULL, 3, 0, NULL),
            (4242, 10, NULL, 55555, 1, 0, NULL),
            -- Taken back. The archive keeps the row and dates it; the thread must
            -- not draw it. Without the filter this is an extra chip on msg 10.
            (4242, 10, '❤️', NULL, 1, 0, '2026-01-01 00:00:00')",
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
    // Newest activity first: Telegram (2023 seconds), then IRC (2020 datetimes),
    // then Signal and Google Chat (1970 milliseconds). IRC ids are looked up.
    // Keyed on (network, target), since `s_20` names two rows. The two empty
    // conversations tie at the end in collection order (`sort_by_key` is stable).
    let irc_id = |network: &str, name: &str| {
        convs
            .iter()
            .find(|c| {
                c.origin == Origin::Irc
                    && c.name.as_deref() == Some(name)
                    && c.network.as_deref() == Some(network)
            })
            .unwrap_or_else(|| panic!("no IRC conversation {network}/{name}"))
            .id
            .clone()
    };
    let ids: Vec<_> = convs.iter().map(|c| c.id.clone()).collect();
    assert_eq!(
        ids,
        [
            "-1000000000055".to_string(),
            "4242".to_string(),
            irc_id("net", "carol"),
            irc_id("net", "#chan"),
            irc_id("xinutec", "s_20"),
            irc_id("euirc", "s_20"),
            "gc1".to_string(),
            "group:g1".to_string(),
            "dm:alice".to_string(),
            "dm:tie".to_string(),
            "dm:receipts".to_string(),
            "gc2".to_string(),
            "-77".to_string(),
        ],
        "sort by last_ts desc"
    );

    // Two rows differing only by network.
    let s20: Vec<_> = convs
        .iter()
        .filter(|c| c.name.as_deref() == Some("s_20"))
        .collect();
    assert_eq!(s20.len(), 2, "the fixture holds one s_20 per network");
    let mut nets: Vec<_> = s20.iter().filter_map(|c| c.network.as_deref()).collect();
    nets.sort_unstable();
    assert_eq!(nets, ["euirc", "xinutec"], "each carries its own network");

    for c in convs.iter().filter(|c| c.origin != Origin::Irc) {
        assert_eq!(c.network, None, "{} has no network", c.id);
    }

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
        (Origin::Gchat, ConversationKind::Dm, 3, Some(8000))
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

    let page = archive::messages_page(&pool, Origin::Signal, "dm:alice", None, 100, PageDir::Older)
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

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let p =
            archive::messages_page(&pool, Origin::Signal, "dm:alice", cursor, 2, PageDir::Older)
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

    // dm:tie's four messages share server_ts 1500; a timestamp-only cursor would
    // lose two of them.
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let p = archive::messages_page(&pool, Origin::Signal, "dm:tie", cursor, 2, PageDir::Older)
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

    let page = archive::messages_page(&pool, Origin::Signal, "dm:alice", None, 100, PageDir::Older)
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

    assert!(page.messages[1].attachments.is_empty());

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

    let page = archive::messages_page(&pool, Origin::Gchat, "gc1", None, 100, PageDir::Older)
        .await
        .unwrap();
    let ts: Vec<_> = page.messages.iter().map(|m| m.ts).collect();
    assert_eq!(ts, [6000, 7000, 8000], "µs→ms, ascending");
    assert!(!page.messages[0].is_outgoing && page.messages[0].sender == "Bob");
    let hey = &page.messages[1];
    assert!(hey.is_outgoing, "is_self → is_outgoing");
    assert_eq!(
        (hey.reactions[0].emoji.as_str(), hey.reactions[0].count),
        ("❤️", 3)
    );
    // Three reacted, two are named: the count stays three.
    assert_eq!(
        hey.reactions[0].who,
        vec!["Bob".to_owned(), "g-carol".to_owned()],
        "named or not, ordered by what the READER sees — ordering by the nullable \
         name column would sort every unnameable reactor to the front"
    );
}

#[tokio::test]
async fn search_spans_origins_finds_deleted_newest_first() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let hits = archive::search(&pool, "findme", 50, archive::SearchScope::Everywhere)
        .await
        .unwrap();
    // Three of the six 'findme' rows. The IRC three must not surface: a server
    // notice, and a notice and a message in the status window.
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

    // A retracted message is found, and flagged.
    let gone = archive::search(&pool, "gone", 50, archive::SearchScope::Everywhere)
        .await
        .unwrap();
    assert_eq!(gone.len(), 1, "the deleted Signal message is findable");
    assert_eq!(gone[0].snippet, "gone");
    assert!(gone[0].deleted, "and the hit says it was retracted");

    // And only that one.
    assert!(
        hits.iter().all(|h| !h.deleted),
        "the live 'findme' hits are not marked deleted"
    );
}

/// `is_status` must filter inside the IRC scan, before `LIMIT`: the newest
/// `findme` is the status window's, so filtering after the join would return a
/// page one short.
#[tokio::test]
async fn search_applies_the_status_exclusion_before_the_limit() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let hits = archive::search(&pool, "findme", 1, archive::SearchScope::Everywhere)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        (hits[0].origin, hits[0].snippet.as_str()),
        (Origin::Irc, "first findme"),
        "the status log's newer 'findme' must not consume the one slot"
    );
}

/// The status window is left out, and only speech is counted.
#[tokio::test]
async fn irc_conversations_leave_out_the_status_log_and_count_only_speech() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let convs = archive::list_conversations(&pool).await.unwrap();
    let irc: Vec<_> = convs.iter().filter(|c| c.origin == Origin::Irc).collect();
    let named: Vec<_> = irc
        .iter()
        .map(|c| (c.network.as_deref(), c.name.as_deref()))
        .collect();
    assert_eq!(
        named,
        [
            (Some("net"), Some("carol")),
            (Some("net"), Some("#chan")),
            (Some("xinutec"), Some("s_20")),
            (Some("euirc"), Some("s_20")),
        ],
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

/// A page carries only speech; an action is marked in `kind`, its body the words
/// alone.
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
    let page = archive::messages_page(&pool, Origin::Irc, &chan.id, None, 50, PageDir::Older)
        .await
        .unwrap();

    let bodies: Vec<_> = page
        .messages
        .iter()
        .map(|m| m.body.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        bodies,
        ["first findme", "second", "waves"],
        "join and notice excluded; the action's body is the words, no star"
    );
    assert_eq!(
        page.messages.iter().map(|m| m.kind).collect::<Vec<_>>(),
        [
            MessageKind::Message,
            MessageKind::Message,
            MessageKind::Action
        ],
        "the kind column reaches the API instead of being folded into the text"
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

/// Three of the channel's lines share a minute, so the id orders them.
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

    // One at a time through a shared timestamp.
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = archive::messages_page(&pool, Origin::Irc, &chan.id, cursor, 1, PageDir::Older)
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
    assert_eq!(seen, ["first findme", "second", "waves"], "file order");
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

    // The status window is not listed, so it is looked up by the fixture's id.
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

/// The row the send path writes and the row the importer later writes for the
/// same log line are one row, by the unique key.
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
        // The tag irssi reported, as the importer takes it from the path.
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

    // The importer's write: the archiver's `insert_irc_line`, INSERT IGNORE on
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

    // The timestamp comes from the log line, not the clock.
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

/// A send irssi could not find in the log records nothing; the importer will.
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

/// `sent_at` is read off the log line, as the importer reads it.
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

    // `< nick>`: the space is the channel mode column.
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

    // Not a log line: record nothing rather than guess a time.
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

// ---- the send guard, through the real router --------------------------------

/// Here rather than in `tests/api_routes.rs`, which touches no archive table:
/// the two binaries share one database and `seed` drops tables.
///
/// `send` checks for a configured sender before looking the conversation up, so
/// this needs one. It never connects.
async fn sending_state(pool: &MySqlPool) -> AppState {
    // TMPDIR can be unreadable under `nix develop`.
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("send-guard");
    let (keys, work) = (root.join("secret"), root.join("run"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&keys).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(keys.join("id_ed25519"), b"not a real key, and never used\n").unwrap();
    std::fs::write(
        keys.join("known_hosts"),
        b"[127.0.0.1]:1 ssh-ed25519 AAAA\n",
    )
    .unwrap();

    let send = messages::config::IrcSend {
        // Port 1: the ordinary half fails fast, after the guard.
        host: "127.0.0.1".to_string(),
        port: 1,
        key_dir: keys.display().to_string(),
        work_dir: work.display().to_string(),
    };
    let sender = IrcSender::prepare(&send).await.unwrap();
    assert!(sender.is_some(), "the fixture key should stage");

    let cfg = Config {
        db_options: sqlx::mysql::MySqlConnectOptions::new(),
        session_secret: SEND_SECRET.to_string(),
        bind_addr: String::new(),
        nc_base_url: "https://nc.invalid".to_string(),
        nc_client_id: String::new(),
        nc_client_secret: String::new(),
        nc_redirect_uri: String::new(),
        allowed_users: vec!["pippijn".to_string()],
        static_dir: None,
        attachments_dir: "/nonexistent".to_string(),
        link_images_dir: "/link-images".into(),
        telegram_media_dir: "/telegram-media".into(),
        link_fetcher_url: "http://link-fetch.invalid".into(),
        irc_send: None,
    };
    AppState::new(pool.clone(), cfg, reqwest::Client::new(), sender)
}

const SEND_SECRET: &str = "test session secret";

async fn send_to(pool: &MySqlPool, conversation_id: i32) -> axum::http::StatusCode {
    use tower::ServiceExt;

    // `seed` drops `sessions` and recreates only the archive tables.
    messages::db::ensure_schema(pool)
        .await
        .expect("sessions table");

    let cookie = messages::session::create_session(
        pool,
        SEND_SECRET,
        &messages::session::UserSession {
            user_id: "pippijn".to_string(),
            display_name: "Pippijn".to_string(),
        },
    )
    .await
    .expect("create session");

    let req = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/api/conversations/irc/{conversation_id}/send"))
        .header(
            "cookie",
            format!("{}={cookie}", messages::session::COOKIE_NAME),
        )
        .header("content-type", "application/json")
        .body(axum::body::Body::from(r#"{"text":"hello"}"#))
        .unwrap();

    messages::routes::router(sending_state(pool).await)
        .oneshot(req)
        .await
        .unwrap()
        .status()
}

/// The status window is refused and an ordinary conversation is not; the
/// second half asserts only "not 404", since the send then fails at ssh.
#[tokio::test]
async fn sending_to_the_status_log_is_refused_and_to_a_conversation_is_not() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };

    let status: i32 =
        sqlx::query_scalar("SELECT id FROM irc_conversations WHERE target='me' AND is_status=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let ordinary: i32 = sqlx::query_scalar("SELECT id FROM irc_conversations WHERE target='carol'")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        send_to(&pool, status).await,
        axum::http::StatusCode::NOT_FOUND,
        "the pseudo-conversation irssi files server notices into must not be sendable"
    );
    assert_ne!(
        send_to(&pool, ordinary).await,
        axum::http::StatusCode::NOT_FOUND,
        "an ordinary conversation must get PAST the guard"
    );
}

// ---- what a leading slash means (no DB) -------------------------------------

/// The send path gives irssi data, never commands, so `/me` is translated here.
#[test]
fn a_leading_slash_means_an_action_an_escape_or_nothing() {
    use messages::irc_send::parse_slash;

    assert_eq!(parse_slash("/me waves"), ("waves", true));
    assert_eq!(parse_slash("/me  padded "), (" padded ", true));

    // IRC's escape for a literal leading slash.
    assert_eq!(parse_slash("//me waves"), ("/me waves", false));
    assert_eq!(parse_slash("//quit"), ("/quit", false));

    // A message starting with a path is ordinary text.
    assert_eq!(
        parse_slash("/usr/bin/foo is broken"),
        ("/usr/bin/foo is broken", false)
    );
    assert_eq!(parse_slash("/quit"), ("/quit", false));

    // A bare `/me` is not an action with an empty body.
    assert_eq!(parse_slash("/me"), ("/me", false));
    assert_eq!(parse_slash("/me   "), ("/me   ", false));

    assert_eq!(parse_slash("ordinary words"), ("ordinary words", false));
}

/// A search hit's cursor, passed to `messages_page`, lands on the hit, for each
/// origin.
#[tokio::test]
async fn a_search_hit_carries_a_cursor_the_pager_accepts() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    for hit in archive::search(&pool, "findme", 50, archive::SearchScope::Everywhere)
        .await
        .unwrap()
    {
        let cursor = archive::parse_cursor(&hit.cursor)
            .unwrap_or_else(|| panic!("{:?} hit minted an unparseable cursor", hit.origin));

        // Strictly older than the hit excludes it.
        let older = archive::messages_page(
            &pool,
            hit.origin,
            &hit.conversation_id,
            Some(cursor),
            50,
            PageDir::Older,
        )
        .await
        .unwrap();
        assert!(
            !older.messages.iter().any(|m| m.ts == hit.ts),
            "{:?}: paging older than the hit returned the hit",
            hit.origin
        );
    }
}

/// Walking forward returns the same messages as walking back, and no others.
#[tokio::test]
async fn paging_forward_mirrors_reading_the_whole_conversation() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    // `gc1` is the Google Chat id; `group:g1` is Signal's. `dm:tie` is what
    // makes this bite: only the id orders its four messages.
    for (origin, cid) in [
        (Origin::Signal, "dm:alice"),
        (Origin::Signal, "dm:tie"),
        (Origin::Gchat, "gc1"),
        (Origin::Irc, "1"),
    ] {
        let whole = archive::messages_page(&pool, origin, cid, None, 1000, PageDir::Older)
            .await
            .unwrap();
        // By id: `dm:tie`'s timestamps are all equal.
        let all: Vec<String> = whole.messages.iter().map(|m| m.id.clone()).collect();
        assert!(all.len() >= 2, "{origin:?}: fixture too small to page");

        // The oldest row starts the walk, so it is seeded by hand.
        let mut fwd = vec![all[0].clone()];
        let mut cursor = whole.next_cursor.as_deref().and_then(parse_cursor);
        while let Some(c) = cursor {
            // One at a time, so every step is a page boundary.
            let p = archive::messages_page(&pool, origin, cid, Some(c), 1, PageDir::Newer)
                .await
                .unwrap();
            if p.messages.is_empty() {
                break;
            }
            fwd.extend(p.messages.iter().map(|m| m.id.clone()));
            if !p.has_more {
                break;
            }
            cursor = p.prev_cursor.as_deref().and_then(parse_cursor);
        }

        assert_eq!(fwd, all, "{origin:?}: forward walk differs from the whole");
    }
}

/// `next_cursor` addresses the oldest row and `prev_cursor` the newest,
/// whichever way the page was fetched.
#[tokio::test]
async fn a_page_addresses_both_of_its_ends() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let whole = archive::messages_page(
        &pool,
        Origin::Signal,
        "dm:alice",
        None,
        1000,
        PageDir::Older,
    )
    .await
    .unwrap();
    let oldest = parse_cursor(whole.next_cursor.as_deref().unwrap()).unwrap();
    let newest = parse_cursor(whole.prev_cursor.as_deref().unwrap()).unwrap();
    assert!(oldest.0 < newest.0, "the two ends are not the same row");

    let before = archive::messages_page(
        &pool,
        Origin::Signal,
        "dm:alice",
        Some(oldest),
        10,
        PageDir::Older,
    )
    .await
    .unwrap();
    assert!(
        before.messages.is_empty(),
        "nothing precedes the first message"
    );

    let after = archive::messages_page(
        &pool,
        Origin::Signal,
        "dm:alice",
        Some(newest),
        10,
        PageDir::Newer,
    )
    .await
    .unwrap();
    assert!(
        after.messages.is_empty(),
        "nothing follows the last message"
    );
}

/// A landing contains the message it landed on. `Older` and `Newer` are both
/// strict, so this needs `AtAndNewer`.
#[tokio::test]
async fn a_landing_contains_the_message_it_landed_on() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    for hit in archive::search(&pool, "findme", 50, archive::SearchScope::Everywhere)
        .await
        .unwrap()
    {
        let cursor = parse_cursor(&hit.cursor).expect("a parseable cursor");
        let older = archive::messages_page(
            &pool,
            hit.origin,
            &hit.conversation_id,
            Some(cursor),
            25,
            PageDir::Older,
        )
        .await
        .unwrap();
        let newer = archive::messages_page(
            &pool,
            hit.origin,
            &hit.conversation_id,
            Some(cursor),
            25,
            PageDir::AtAndNewer,
        )
        .await
        .unwrap();

        // By id: IRC rows share timestamps.
        let want = cursor.1.to_string();
        let landed: Vec<&str> = older
            .messages
            .iter()
            .chain(newer.messages.iter())
            .map(|m| m.id.as_str())
            .collect();
        assert!(
            landed.contains(&want.as_str()),
            "{:?}: the landing is missing the hit (row {want}) — loaded {landed:?}",
            hit.origin
        );
        // Exactly once.
        assert_eq!(
            landed.iter().filter(|&&i| i == want).count(),
            1,
            "{:?}: the hit appears more than once",
            hit.origin
        );
    }
}

// ---- an edited message is one message --------------------------------------

/// A thread per test, since they run in parallel. No `conversations` row, so
/// the fixture's lists are untouched.
async fn seed_edits(pool: &MySqlPool, thread: &str) {
    // An original, two revisions, and a message after.
    sqlx::query("DELETE FROM messages WHERE thread_id = ?")
        .bind(thread)
        .execute(pool)
        .await
        .unwrap();
    for (ts, body, edit_of, edited) in [
        (1_000i64, "first thought", None::<i64>, 1i8),
        (2_000, "second thought", Some(1_000), 0),
        (3_000, "what it says now", Some(1_000), 0),
        (4_000, "a later message", None, 0),
    ] {
        sqlx::query(
            "INSERT INTO messages (thread_id, sender_uuid, server_ts, body, is_outgoing, deleted, edited, edit_of_ts)
             VALUES (?, 'u1', ?, ?, 0, 0, ?, ?)",
        )
        .bind(thread)
        .bind(ts)
        .bind(body)
        .bind(edited)
        .bind(edit_of)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// A revision row is not drawn as a second message.
#[tokio::test]
async fn an_edited_message_is_one_message_saying_what_it_says_now() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let thread = "dm:edits-one";
    seed_edits(&pool, thread).await;
    let page = archive::messages_page(&pool, Origin::Signal, thread, None, 50, PageDir::Older)
        .await
        .unwrap();

    assert_eq!(
        page.messages.len(),
        2,
        "revisions are versions, not things said"
    );
    let edited = &page.messages[0];
    assert_eq!(edited.ts, 1_000, "it stays where it was said");
    assert_eq!(
        edited.body.as_deref(),
        Some("what it says now"),
        "and reads as it reads now"
    );
    assert!(edited.edited);
    assert_eq!(page.messages[1].body.as_deref(), Some("a later message"));
}

/// Each version is dated by the row before it: it was current until the next
/// arrived.
#[tokio::test]
async fn the_history_is_every_earlier_version_in_order() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let thread = "dm:edits-history";
    seed_edits(&pool, thread).await;
    let page = archive::messages_page(&pool, Origin::Signal, thread, None, 50, PageDir::Older)
        .await
        .unwrap();
    let history: Vec<(i64, &str)> = page.messages[0]
        .edits
        .iter()
        .map(|e| (e.ts, e.body.as_deref().unwrap_or("")))
        .collect();
    assert_eq!(
        history,
        [(1_000, "first thought"), (2_000, "second thought")]
    );
}

#[tokio::test]
async fn a_message_nobody_edited_has_no_history() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let thread = "dm:edits-none";
    seed_edits(&pool, thread).await;
    let page = archive::messages_page(&pool, Origin::Signal, thread, None, 50, PageDir::Older)
        .await
        .unwrap();
    assert!(page.messages[1].edits.is_empty());
}

/// Telegram conversations keep their stored kind, `channel` included, and the
/// count leaves out service events.
#[tokio::test]
async fn telegram_conversations_keep_their_kind_and_count_only_speech() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };
    let convs = archive::list_conversations(&pool).await.unwrap();
    let tg = |id: &str| {
        convs
            .iter()
            .find(|c| c.origin == Origin::Telegram && c.id == id)
            .unwrap_or_else(|| panic!("no Telegram conversation {id}"))
    };

    let dm = tg("4242");
    assert_eq!(dm.kind, ConversationKind::Dm);
    assert_eq!(dm.name.as_deref(), Some("Tessa"));
    assert_eq!(dm.message_count, 6, "the service event must not be counted");
    // The newest speech, in milliseconds; msg 16 follows the service event.
    assert_eq!(dm.last_ts, Some(1_700_000_260_000));

    assert_eq!(tg("-1000000000055").kind, ConversationKind::Channel);
    assert_eq!(tg("-77").kind, ConversationKind::Group);
    assert_eq!(tg("-77").message_count, 0);
    assert_eq!(tg("-77").last_ts, None);
}

/// A Telegram page: oldest first, service events as actions, and the two
/// messages sharing a second both present in a stable order.
#[tokio::test]
async fn a_telegram_page_is_speech_in_order_including_a_shared_second() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };
    let page = archive::messages_page(&pool, Origin::Telegram, "4242", None, 3, PageDir::Older)
        .await
        .unwrap();
    let bodies: Vec<Option<&str>> = page.messages.iter().map(|m| m.body.as_deref()).collect();
    assert_eq!(
        bodies,
        vec![
            Some("forget it"),
            Some("changed the photo"),
            Some("telegram touched this")
        ]
    );
    assert!(page.has_more);

    let older = archive::messages_page(
        &pool,
        Origin::Telegram,
        "4242",
        archive::parse_cursor(page.next_cursor.as_deref().unwrap()),
        3,
        PageDir::Older,
    )
    .await
    .unwrap();
    let bodies: Vec<Option<&str>> = older.messages.iter().map(|m| m.body.as_deref()).collect();
    assert_eq!(
        bodies,
        vec![Some("ook hoi"), Some("same second"), Some("third go")],
        "both messages of the shared second, once each"
    );

    // This boundary sits on 1700000060 with another row in the same second.
    let oldest = archive::messages_page(
        &pool,
        Origin::Telegram,
        "4242",
        archive::parse_cursor(older.next_cursor.as_deref().unwrap()),
        3,
        PageDir::Older,
    )
    .await
    .unwrap();
    let bodies: Vec<Option<&str>> = oldest.messages.iter().map(|m| m.body.as_deref()).collect();
    assert_eq!(
        bodies,
        vec![Some("hoi")],
        "the row above the boundary shares its second and must not come back"
    );
    assert!(!oldest.has_more, "that is the start of the conversation");
}

/// Album members carry Telegram's `grouped_id`, as a string: it exceeds what a
/// JavaScript number holds exactly.
#[tokio::test]
async fn a_telegram_album_member_carries_its_album_id() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };
    let page = archive::messages_page(&pool, Origin::Telegram, "4242", None, 50, PageDir::Older)
        .await
        .unwrap();
    let albums: Vec<(Option<&str>, Option<&str>)> = page
        .messages
        .iter()
        .map(|m| (m.body.as_deref(), m.album.as_deref()))
        .collect();
    assert_eq!(
        albums,
        vec![
            (Some("hoi"), None),
            (Some("ook hoi"), None),
            (Some("same second"), Some("7777777777777777777")),
            (Some("third go"), Some("7777777777777777777")),
            (Some("forget it"), None),
            (Some("changed the photo"), None),
            (Some("telegram touched this"), None),
        ]
    );

    let signal =
        archive::messages_page(&pool, Origin::Signal, "dm:alice", None, 50, PageDir::Older)
            .await
            .unwrap();
    assert!(signal.messages.iter().all(|m| m.album.is_none()));
}

/// Telegram's edit history, the original first: it carries no edit date.
#[tokio::test]
async fn a_telegram_edit_history_starts_with_what_was_said_first() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };
    let page = archive::messages_page(&pool, Origin::Telegram, "4242", None, 50, PageDir::Older)
        .await
        .unwrap();
    let edited = page
        .messages
        .iter()
        .find(|m| m.body.as_deref() == Some("third go"))
        .expect("the edited message");
    assert!(edited.edited);
    assert_eq!(
        edited
            .edits
            .iter()
            .map(|e| e.body.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("first go"), Some("second go")]
    );
    // The original is dated by the message; the next by its edit date.
    assert_eq!(edited.edits[0].ts, 1_700_000_120_000);
    assert_eq!(edited.edits[1].ts, 1_700_000_400_000);

    let plain = page
        .messages
        .iter()
        .find(|m| m.body.as_deref() == Some("hoi"))
        .expect("an unedited message");
    assert!(!plain.edited);
    assert!(plain.edits.is_empty());
}

/// Only drawable, current reactions reach the reader, and a retracted message
/// keeps its words.
#[tokio::test]
async fn telegram_reactions_are_drawable_and_a_retraction_keeps_its_words() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };
    let page = archive::messages_page(&pool, Origin::Telegram, "4242", None, 50, PageDir::Older)
        .await
        .unwrap();

    let reacted = page
        .messages
        .iter()
        .find(|m| m.body.as_deref() == Some("hoi"))
        .expect("the reacted message");
    // One of three stored: the custom emoji cannot be drawn, and the other was
    // taken back.
    assert_eq!(reacted.reactions.len(), 1);
    assert_eq!(reacted.reactions[0].emoji, "👍");
    assert_eq!(reacted.reactions[0].count, 3);

    let gone = page
        .messages
        .iter()
        .find(|m| m.deleted)
        .expect("the retracted message");
    assert_eq!(
        gone.body.as_deref(),
        Some("forget it"),
        "the server sends retracted text and the reader hides it — the archive-wide policy"
    );
}

/// A Telegram search hit's cursor is in seconds and lands on the hit.
#[tokio::test]
async fn a_telegram_search_hit_lands_on_its_own_message() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };
    let hits = archive::search(&pool, "third go", 20, archive::SearchScope::Everywhere)
        .await
        .unwrap();
    let hit = hits
        .iter()
        .find(|h| h.origin == Origin::Telegram)
        .expect("a Telegram hit");
    assert_eq!(hit.conversation_id, "4242");
    assert_eq!(hit.conversation_name.as_deref(), Some("Tessa"));
    assert_eq!(hit.ts, 1_700_000_120_000);

    let landed = archive::messages_page(
        &pool,
        Origin::Telegram,
        &hit.conversation_id,
        archive::parse_cursor(&hit.cursor),
        5,
        PageDir::AtAndNewer,
    )
    .await
    .unwrap();
    assert_eq!(
        landed.messages.first().map(|m| m.body.as_deref()),
        Some(Some("third go")),
        "a landing must include the message it landed on, not its neighbour"
    );
}

/// A non-numeric conversation id pages as empty.
#[tokio::test]
async fn a_non_numeric_telegram_id_is_an_empty_page() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };
    let page = archive::messages_page(
        &pool,
        Origin::Telegram,
        "nonsense",
        None,
        10,
        PageDir::Older,
    )
    .await
    .unwrap();
    assert!(page.messages.is_empty());
    assert!(!page.has_more);
}

/// `edit_hide` suppresses the edited mark; an ordinary edit keeps it.
#[tokio::test]
async fn a_hidden_telegram_edit_is_not_shown_as_edited() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };
    let page = archive::messages_page(&pool, Origin::Telegram, "4242", None, 50, PageDir::Older)
        .await
        .unwrap();
    let by = |body: &str| {
        page.messages
            .iter()
            .find(|m| m.body.as_deref() == Some(body))
            .unwrap_or_else(|| panic!("no message {body:?}"))
    };

    let hidden = by("telegram touched this");
    assert!(
        !hidden.edited,
        "Telegram asked for this one to read as unmodified"
    );
    assert!(
        hidden.edits.is_empty(),
        "and no history panel is offered for it either"
    );

    assert!(by("third go").edited);
}

/// Telegram media arrives as `attachments`: held files are `available`, files
/// still at Telegram are not.
#[tokio::test]
async fn telegram_media_arrives_as_attachments_with_availability() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };
    let page = archive::messages_page(&pool, Origin::Telegram, "4242", None, 50, PageDir::Older)
        .await
        .unwrap();
    let by = |body: &str| {
        page.messages
            .iter()
            .find(|m| m.body.as_deref() == Some(body))
            .unwrap_or_else(|| panic!("no message {body:?}"))
    };

    let held = by("forget it");
    assert_eq!(held.attachments.len(), 1);
    assert!(held.attachments[0].available);
    assert!(held.attachments[0].is_image);
    assert_eq!(held.attachments[0].size, Some(204_800));
    // The message's API id, which the media route takes.
    assert_eq!(held.attachments[0].id, held.id);

    // msg 13: offered, not held, and not an image.
    let offered = by("third go");
    assert_eq!(offered.attachments.len(), 1);
    assert!(
        !offered.attachments[0].available,
        "1.5GB still at Telegram must not be drawn as a picture"
    );
    assert!(!offered.attachments[0].is_image);

    assert!(by("hoi").attachments.is_empty());
}

/// A request queues only what is not stored or already wanted, and reports
/// whether it did.
#[tokio::test]
async fn requesting_media_queues_only_what_is_not_held() {
    let Some(pool) = seeded_pool().await else {
        eprintln!("skipping: MESSAGES_TEST_DATABASE_URL not set");
        return;
    };
    let page = archive::messages_page(&pool, Origin::Telegram, "4242", None, 50, PageDir::Older)
        .await
        .unwrap();
    let id_of = |body: &str| {
        page.messages
            .iter()
            .find(|m| m.body.as_deref() == Some(body))
            .unwrap_or_else(|| panic!("no message {body:?}"))
            .id
            .parse::<i64>()
            .expect("a numeric api id")
    };

    let video = id_of("third go");
    assert!(archive::request_telegram_media(&pool, video).await.unwrap());
    assert!(
        !archive::request_telegram_media(&pool, video).await.unwrap(),
        "a second tap must not re-queue it"
    );

    let page = archive::messages_page(&pool, Origin::Telegram, "4242", None, 50, PageDir::Older)
        .await
        .unwrap();
    let after = page
        .messages
        .iter()
        .find(|m| m.body.as_deref() == Some("third go"))
        .expect("the message");
    assert_eq!(
        after.attachments[0].fetch,
        Some(archive::FetchState::Wanted)
    );
    assert!(!after.attachments[0].available);

    let photo = id_of("forget it");
    assert!(
        !archive::request_telegram_media(&pool, photo).await.unwrap(),
        "a stored file must not be re-queued"
    );
}

// ---- what a message was a reply to ------------------------------------------

/// Signal's quote names a timestamp: resolved, deleted, or not held. An
/// unresolved quote is an ordinary result.
#[tokio::test]
async fn a_signal_quote_resolves_withholds_a_deletion_and_survives_a_miss() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let page = archive::messages_page(&pool, Origin::Signal, "dm:alice", None, 100, PageDir::Older)
        .await
        .unwrap();
    let by_ts = |ts: i64| {
        page.messages
            .iter()
            .find(|m| m.ts == ts)
            .unwrap_or_else(|| panic!("no message at {ts}"))
    };

    let r = by_ts(2000).reply_to.as_ref().expect("2000 quotes 1000");
    assert_eq!(r.ts, Some(1000));
    assert_eq!(r.sender.as_deref(), Some("Alice"));
    assert_eq!(r.excerpt.as_deref(), Some("hi"));
    assert!(!r.deleted);
    // The id and the cursor come together.
    let id = r.id.as_deref().expect("held → an id to go to");
    let cursor = r.cursor.as_deref().expect("held → a cursor to land on");
    assert_eq!(parse_cursor(cursor), Some((1000, id.parse().unwrap())));

    // The target is deleted, so its words are withheld.
    let r = by_ts(3000).reply_to.as_ref().expect("3000 quotes 4000");
    assert!(r.deleted, "the quoted message was deleted");
    assert_eq!(r.excerpt, None, "a deleted message is not quoted verbatim");
    assert!(r.id.is_some(), "still somewhere to go");

    // Nothing at ts=999; the timestamp is still reported.
    let r = by_ts(1000).reply_to.as_ref().expect("1000 quotes 999");
    assert_eq!((r.id.as_ref(), r.cursor.as_ref()), (None, None));
    assert_eq!(r.ts, Some(999), "when, even with no what");
    assert_eq!(r.excerpt, None);

    assert!(by_ts(4000).reply_to.is_none());
}

/// Telegram's reply names an id, and can target a service event.
#[tokio::test]
async fn a_telegram_reply_resolves_and_refuses_to_point_at_a_service_event() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let page = archive::messages_page(&pool, Origin::Telegram, "4242", None, 100, PageDir::Older)
        .await
        .unwrap();
    let by_body = |body: &str| {
        page.messages
            .iter()
            .find(|m| m.body.as_deref() == Some(body))
            .unwrap_or_else(|| panic!("no message {body:?}"))
            .reply_to
            .clone()
    };

    let r = by_body("ook hoi").expect("11 replies to 10");
    assert_eq!(r.sender.as_deref(), Some("Tessa"));
    assert_eq!(r.excerpt.as_deref(), Some("hoi"));
    // The cursor in Telegram's seconds; `ts` in milliseconds.
    let (cur_ts, _) = parse_cursor(r.cursor.as_deref().unwrap()).unwrap();
    assert_eq!(cur_ts, 1_700_000_000, "seconds, not milliseconds");
    assert_eq!(r.ts, Some(1_700_000_000_000), "milliseconds on the wire");

    let r = by_body("same second").expect("12 replies to 14");
    assert!(r.deleted && r.excerpt.is_none(), "deleted target, no words");

    // A reply to a service event resolves, since the page returns the event.
    let r = by_body("third go").expect("13 replies to the service event 15");
    assert!(
        r.id.is_some() && r.cursor.is_some() && r.ts.is_some(),
        "the event is on a page now, so it is a destination"
    );
    assert_eq!(r.excerpt.as_deref(), Some("changed the photo"));

    // No message 999, and a Telegram reply names no time.
    let r = by_body("telegram touched this").expect("16 replies to 999");
    assert_eq!((r.id.as_ref(), r.ts), (None, None));

    assert!(by_body("hoi").is_none(), "10 replied to nothing");
}

/// IRC records no reply association.
#[tokio::test]
async fn irc_carries_no_reply() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let page = archive::messages_page(&pool, Origin::Irc, "1", None, 100, PageDir::Older)
        .await
        .unwrap();
    assert!(!page.messages.is_empty());
    assert!(
        page.messages.iter().all(|m| m.reply_to.is_none()),
        "a log line answers nothing, and the format has no way to say otherwise"
    );
}

// ---- who has read how far ---------------------------------------------------

/// Telegram read state: read, sent, or `None` when the archive cannot say.
#[tokio::test]
async fn telegram_read_state_is_mine_only_and_silent_without_a_mark() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let page = archive::messages_page(&pool, Origin::Telegram, "4242", None, 100, PageDir::Older)
        .await
        .unwrap();
    let state_of = |body: &str| {
        page.messages
            .iter()
            .find(|m| m.body.as_deref() == Some(body))
            .unwrap_or_else(|| panic!("no message {body:?}"))
            .delivery
            .as_ref()
            .map(|d| d.state)
    };

    assert_eq!(state_of("ook hoi"), Some(DeliveryState::Read));

    // Mine, after the mark: sent. Would be read if the inbox mark (16) were used.
    assert_eq!(state_of("telegram touched this"), Some(DeliveryState::Sent));

    // Theirs: `None`.
    assert_eq!(state_of("hoi"), None);
    assert_eq!(state_of("same second"), None);

    // Telegram cannot report `Delivered`.
    assert!(
        page.messages
            .iter()
            .all(|m| m.delivery.as_ref().map(|d| d.state) != Some(DeliveryState::Delivered)),
        "Telegram cannot report delivery"
    );

    assert!(
        page.messages
            .iter()
            .all(|m| m.delivery.as_ref().is_none_or(|d| d.read_by.is_empty())),
        "a read mark carries a position, never a person"
    );
}

/// A conversation with no mark says nothing.
#[tokio::test]
async fn a_conversation_with_no_read_mark_reports_nothing() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let page = archive::messages_page(
        &pool,
        Origin::Telegram,
        "-1000000000055",
        None,
        100,
        PageDir::Older,
    )
    .await
    .unwrap();
    assert!(!page.messages.is_empty(), "the channel has a message");
    assert!(
        page.messages.iter().all(|m| m.delivery.is_none()),
        "no mark → no claim, in either direction"
    );
}

/// Google Chat and IRC carry no read state.
#[tokio::test]
async fn the_other_origins_report_no_read_state() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    for (origin, id) in [(Origin::Gchat, "gc1")] {
        let page = archive::messages_page(&pool, origin, id, None, 100, PageDir::Older)
            .await
            .unwrap();
        assert!(!page.messages.is_empty(), "{origin:?} has messages");
        assert!(
            page.messages.iter().all(|m| m.delivery.is_none()),
            "{origin:?} records no read state"
        );
    }
}

// ---- Signal: who read it, and when ------------------------------------------

/// Signal's ladder: delivered and read are distinct, readers are named, and my
/// own linked device is not one of them.
#[tokio::test]
async fn signal_receipts_climb_the_ladder_and_name_the_reader() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let page = archive::messages_page(
        &pool,
        Origin::Signal,
        "dm:receipts",
        None,
        100,
        PageDir::Older,
    )
    .await
    .unwrap();
    let msg = |body: &str| {
        page.messages
            .iter()
            .find(|m| m.body.as_deref() == Some(body))
            .unwrap_or_else(|| panic!("no message {body:?}"))
    };
    let state_of = |body: &str| msg(body).delivery.as_ref().map(|d| d.state);

    // Delivered and read: the higher wins.
    assert_eq!(state_of("read by her"), Some(DeliveryState::Read));

    // Delivered, not read.
    assert_eq!(state_of("delivered only"), Some(DeliveryState::Delivered));

    // No receipt, but we were listening.
    assert_eq!(state_of("nothing back"), Some(DeliveryState::Sent));

    // Before capture began (900 < 1000): nothing to say.
    assert_eq!(state_of("before we listened"), None);

    // Hers: `None`, despite my read sync targeting it.
    assert_eq!(state_of("her words"), None);

    // Named, without my own read sync.
    let read_by: Vec<_> = msg("read by her")
        .delivery
        .as_ref()
        .unwrap()
        .read_by
        .iter()
        .map(|r| (r.who.as_str(), r.at))
        .collect();
    assert_eq!(read_by, [("Alice", 1220)], "her read, at her timestamp");

    // A delivery names nobody.
    assert!(
        msg("delivered only")
            .delivery
            .as_ref()
            .unwrap()
            .read_by
            .is_empty()
    );
}

/// Google Chat quote-replies resolve, and an unresolvable one still renders as
/// a reply.
#[tokio::test]
async fn a_gchat_quote_reply_points_at_the_message_it_answers() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let page = archive::messages_page(&pool, Origin::Gchat, "gc1", None, 100, PageDir::Older)
        .await
        .unwrap();
    let by_body = |b: &str| {
        page.messages
            .iter()
            .find(|m| m.body.as_deref() == Some(b))
            .unwrap_or_else(|| panic!("no message {b:?}"))
    };

    let answered = by_body("hey");
    let r = answered.reply_to.clone().expect("m2 quote-replies m1");
    assert_eq!(r.sender.as_deref(), Some("Bob"));
    assert_eq!(r.excerpt.as_deref(), Some("hello findme"));
    assert!(r.id.is_some(), "the target is held, so it is a destination");
    // The cursor in Google Chat's microseconds; `ts` in milliseconds.
    let (cur_ts, _) = archive::parse_cursor(r.cursor.as_deref().unwrap()).unwrap();
    assert_eq!(cur_ts, 6_000_000, "µs, the page query's unit");
    assert_eq!(r.ts, Some(6000), "ms, the API's unit");

    let orphan = by_body("answering something missing");
    let r = orphan
        .reply_to
        .clone()
        .expect("a reply we cannot resolve is still a reply");
    assert_eq!(
        (r.id, r.cursor, r.excerpt),
        (None, None, None),
        "nothing to point at, but the message still shows it answered something"
    );
}

/// Google Chat attachments render whether or not their bytes are held.
#[tokio::test]
async fn a_gchat_picture_is_shown_even_when_its_bytes_are_not_held() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let page = archive::messages_page(&pool, Origin::Gchat, "gc1", None, 100, PageDir::Older)
        .await
        .unwrap();
    let hey = page
        .messages
        .iter()
        .find(|m| m.body.as_deref() == Some("hey"))
        .expect("m2");
    assert_eq!(hey.attachments.len(), 2, "both, held or not");

    let held = &hey.attachments[0];
    assert_eq!(held.file_name.as_deref(), Some("holiday.jpg"));
    assert!(
        held.available,
        "stored_path is set, so the bytes are servable"
    );
    assert!(held.is_image);

    let known = &hey.attachments[1];
    assert_eq!(known.file_name.as_deref(), Some("lost.webp"));
    assert!(
        !known.available,
        "no bytes — but the archive still says a picture was here"
    );
    // Nothing a reader could ask for.
    assert!(known.fetch.is_none());
}

// ---- searching inside one conversation --------------------------------------

/// The scope is in the SQL. At `limit = 1`, a global search's hit is IRC's, so
/// filtering it afterwards for Google Chat would find nothing.
#[tokio::test]
async fn a_scoped_search_is_not_a_global_search_filtered_afterwards() {
    let Some(pool) = seeded_pool().await else {
        return;
    };

    let global = archive::search(&pool, "findme", 1, archive::SearchScope::Everywhere)
        .await
        .unwrap();
    assert_eq!(global.len(), 1);
    assert_eq!(global[0].origin, Origin::Irc, "newest wins globally");

    let scoped = archive::search(
        &pool,
        "findme",
        1,
        archive::SearchScope::Conversation {
            origin: Origin::Gchat,
            id: "gc1",
        },
    )
    .await
    .unwrap();
    assert_eq!(
        scoped.len(),
        1,
        "the hit a client-side filter would have lost"
    );
    assert_eq!(scoped[0].origin, Origin::Gchat);
    assert_eq!(scoped[0].conversation_id, "gc1");
}

/// Origins outside the scope are not queried.
#[tokio::test]
async fn a_scope_admits_only_its_own_origin() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    for (origin, id, expect_any) in [
        (Origin::Gchat, "gc1", true),
        // An id that exists, in another origin.
        (Origin::Gchat, "dm:alice", false),
        (Origin::Signal, "group:g1", true),
    ] {
        let hits = archive::search(
            &pool,
            "findme",
            50,
            archive::SearchScope::Conversation { origin, id },
        )
        .await
        .unwrap();
        assert!(
            hits.iter()
                .all(|h| h.origin == origin && h.conversation_id == id),
            "{origin:?}/{id} leaked a hit from elsewhere"
        );
        assert_eq!(!hits.is_empty(), expect_any, "{origin:?}/{id}");
    }
}

/// A conversation without the term returns nothing, not the global result.
#[tokio::test]
async fn a_scope_with_no_match_is_empty_not_global() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let hits = archive::search(
        &pool,
        "findme",
        50,
        archive::SearchScope::Conversation {
            origin: Origin::Signal,
            id: "dm:alice",
        },
    )
    .await
    .unwrap();
    assert!(
        hits.is_empty(),
        "dm:alice has no 'findme'; got {} hit(s) from {:?}",
        hits.len(),
        hits.iter()
            .map(|h| h.conversation_id.as_str())
            .collect::<Vec<_>>()
    );
}

// ---- jumping to a date ------------------------------------------------------

/// Each origin's cursor for a day is in its own unit.
#[test]
fn a_day_cursor_is_minted_in_each_origins_own_unit() {
    // 2026-01-02T00:00:00Z in milliseconds.
    let ms = 1_767_312_000_000i64;
    assert_eq!(
        archive::cursor_for_day(Origin::Signal, ms),
        "1767312000000_0"
    );
    // The only one that multiplies.
    assert_eq!(
        archive::cursor_for_day(Origin::Gchat, ms),
        "1767312000000000_0"
    );
    assert_eq!(
        archive::cursor_for_day(Origin::Telegram, ms),
        "1767312000_0"
    );
    assert_eq!(archive::cursor_for_day(Origin::Irc, ms), "1767312000_0");
}

/// The id is 0, a floor, so no message in the landing second is skipped.
#[test]
fn a_day_cursor_admits_the_whole_first_second() {
    let (ts, id) = archive::parse_cursor(&archive::cursor_for_day(Origin::Irc, 1_767_312_000_000))
        .expect("round-trips");
    assert_eq!(ts, 1_767_312_000);
    assert_eq!(id, 0, "a floor under every real id");
}

/// Landing on a day returns that day's messages, reading forwards.
#[tokio::test]
async fn a_day_lands_on_that_day_and_reads_forwards() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    // IRC is the fixture with real dates.
    let day_ms = 1_577_836_800_000i64; // 2020-01-01T00:00:00Z
    let cursor = archive::parse_cursor(&archive::cursor_for_day(Origin::Irc, day_ms));
    let id = sqlx::query_scalar::<_, i32>(
        "SELECT id FROM irc_conversations WHERE target = '#chan' AND is_status = 0",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let page = archive::messages_page(
        &pool,
        Origin::Irc,
        &id.to_string(),
        cursor,
        100,
        PageDir::AtAndNewer,
    )
    .await
    .unwrap();
    assert!(!page.messages.is_empty(), "the day has messages");
    assert!(
        page.messages.iter().all(|m| m.ts >= day_ms),
        "nothing from BEFORE the day asked for"
    );
    let mut sorted = page.messages.iter().map(|m| m.ts).collect::<Vec<_>>();
    sorted.sort_unstable();
    assert_eq!(
        page.messages.iter().map(|m| m.ts).collect::<Vec<_>>(),
        sorted,
        "oldest first"
    );
}

/// A retracted formatting run is not drawn.
#[tokio::test]
async fn telegram_formatting_is_attached_and_a_retracted_run_is_not() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let page = archive::messages_page(&pool, Origin::Telegram, "4242", None, 100, PageDir::Older)
        .await
        .unwrap();
    let m = page
        .messages
        .iter()
        .find(|m| m.body.as_deref() == Some("ook hoi"))
        .expect("msg 11 is on the page");

    let kinds: Vec<_> = m.entities.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["bold", "textUrl"], "the italic run was retracted");

    // Ordered by offset, for the reader's single pass.
    let offsets: Vec<_> = m.entities.iter().map(|e| e.offset).collect();
    let mut sorted = offsets.clone();
    sorted.sort_unstable();
    assert_eq!(offsets, sorted, "entities arrive in offset order");

    assert_eq!(
        m.entities[1].url.as_deref(),
        Some("https://example.invalid/a"),
        "a textUrl carries where it points; the text alone does not say"
    );

    // No other origin records formatting.
    for (origin, id) in [(Origin::Signal, "dm:alice"), (Origin::Gchat, "gc1")] {
        let p = archive::messages_page(&pool, origin, id, None, 100, PageDir::Older)
            .await
            .unwrap();
        assert!(
            p.messages.iter().all(|m| m.entities.is_empty()),
            "{origin:?} records no formatting"
        );
    }
}

// ---- Signal text styles, link previews, quote details -----------------------

/// A thread of its own: a styled message, a link with a preview, a reply to a
/// message the archive does not hold, and an edited message whose styles
/// changed with its text.
async fn seed_signal_fields(pool: &MySqlPool, thread: &str) {
    sqlx::query("DELETE FROM messages WHERE thread_id = ?")
        .bind(thread)
        .execute(pool)
        .await
        .unwrap();
    for (ts, body, edit_of, edited) in [
        (10_000i64, "Hello bold mono strike", None::<i64>, 0i8),
        (11_000, "https://xinutec.org", None, 0),
        (12_000, "This is many styles.", None, 0),
        (13_000, "plain", None, 1),
        (14_000, "now italic", Some(13_000), 0),
    ] {
        sqlx::query(
            "INSERT INTO messages (thread_id, sender_uuid, server_ts, body, is_outgoing, deleted, edited, edit_of_ts)
             VALUES (?, 'me', ?, ?, 1, 0, ?, ?)",
        )
        .bind(thread)
        .bind(ts)
        .bind(body)
        .bind(edited)
        .bind(edit_of)
        .execute(pool)
        .await
        .unwrap();
    }
    for (ts, style, start, length) in [
        (10_000i64, "BOLD", 6, 4),
        (10_000, "MONOSPACE", 11, 11),
        (10_000, "STRIKETHROUGH", 11, 11),
        (13_000, "BOLD", 0, 5),
        (14_000, "ITALIC", 4, 6),
    ] {
        sqlx::query(
            "INSERT INTO signal_text_styles (message_id, style, start_utf16, length_utf16)
             SELECT id, ?, ?, ? FROM messages WHERE thread_id = ? AND server_ts = ?",
        )
        .bind(style)
        .bind(start)
        .bind(length)
        .bind(thread)
        .bind(ts)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO signal_link_previews (message_id, position, url, title, description)
         SELECT id, 0, 'https://xinutec.org', 'Welcome to nginx!', NULL
           FROM messages WHERE thread_id = ? AND server_ts = 11000",
    )
    .bind(thread)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE messages SET quote_target_ts = 5, quote_author_uuid = 'alice',
                quote_text = 'something from before the archive'
          WHERE thread_id = ? AND server_ts = 12000",
    )
    .bind(thread)
    .execute(pool)
    .await
    .unwrap();
}

/// Signal's style names arrive as the viewer's kinds, overlapping runs kept; an
/// edited message carries its current revision's styles.
#[tokio::test]
async fn signal_styles_arrive_as_entities() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let thread = "dm:fields-styles";
    seed_signal_fields(&pool, thread).await;
    let page = archive::messages_page(&pool, Origin::Signal, thread, None, 50, PageDir::Older)
        .await
        .unwrap();
    let runs = |i: usize| -> Vec<(String, i64, i64)> {
        page.messages[i]
            .entities
            .iter()
            .map(|e| (e.kind.clone(), e.offset, e.length))
            .collect()
    };
    assert_eq!(
        runs(0),
        vec![
            ("bold".into(), 6, 4),
            ("code".into(), 11, 11),
            ("strike".into(), 11, 11),
        ]
    );
    assert!(runs(1).is_empty());
    let edited = &page.messages[3];
    assert_eq!(edited.body.as_deref(), Some("now italic"));
    assert_eq!(runs(3), vec![("italic".into(), 4, 6)]);
}

/// A preview comes with its message.
#[tokio::test]
async fn a_signal_link_preview_comes_with_its_message() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let thread = "dm:fields-previews";
    seed_signal_fields(&pool, thread).await;
    let page = archive::messages_page(&pool, Origin::Signal, thread, None, 50, PageDir::Older)
        .await
        .unwrap();
    let previews: Vec<(&str, Option<&str>)> = page.messages[1]
        .previews
        .iter()
        .map(|p| (p.url.as_str(), p.title.as_deref()))
        .collect();
    assert_eq!(
        previews,
        vec![("https://xinutec.org", Some("Welcome to nginx!"))]
    );
    assert!(page.messages[0].previews.is_empty());
}

/// A reply to a message the archive does not hold shows who and what from the
/// quote itself.
#[tokio::test]
async fn an_unresolved_signal_quote_shows_what_it_quoted() {
    let Some(pool) = seeded_pool().await else {
        return;
    };
    let thread = "dm:fields-quote";
    seed_signal_fields(&pool, thread).await;
    let page = archive::messages_page(&pool, Origin::Signal, thread, None, 50, PageDir::Older)
        .await
        .unwrap();
    let r = page.messages[2].reply_to.as_ref().expect("a reply");
    assert_eq!(r.id, None, "not held");
    assert_eq!(r.sender.as_deref(), Some("Alice"));
    assert_eq!(
        r.excerpt.as_deref(),
        Some("something from before the archive")
    );
}
