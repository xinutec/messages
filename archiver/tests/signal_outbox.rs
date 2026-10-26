//! The Signal outbox: what the ingester claims, what it reads for a quote, and
//! how a send's outcome lands.
//!
//! Skips when `SIGNAL_TEST_DATABASE_URL` is unset, and refuses to skip in CI.

use signal_archiver::db::{Db, SignalQuote};
use signal_archiver::parse::{Message, ThreadId};
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};

async fn connect() -> Option<(Db, MySqlPool)> {
    let Ok(url) = std::env::var("SIGNAL_TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "SIGNAL_TEST_DATABASE_URL is unset in CI: the outbox would skip \
             and would ship unverified"
        );
        eprintln!("SIGNAL_TEST_DATABASE_URL unset — skipping");
        return None;
    };
    let db = Db::connect(&url).await.expect("migrations apply");
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to SIGNAL_TEST_DATABASE_URL");
    Some((db, pool))
}

fn said(thread: &ThreadId, sender: &str, ts: i64, body: &str, outgoing: bool) -> Message {
    Message {
        thread_id: thread.clone(),
        sender: sender.to_owned(),
        server_ts: ts,
        server_received_ts: None,
        server_delivered_ts: None,
        expires_in_seconds: None,
        body: Some(body.to_owned()),
        quote_target_ts: None,
        quote_author: None,
        quote_text: None,
        is_outgoing: outgoing,
        attachments: Vec::new(),
        styles: Vec::new(),
        mentions: Vec::new(),
        previews: Vec::new(),
        group_call: None,
    }
}

async fn queue(
    pool: &MySqlPool,
    thread: &str,
    text: &str,
    quote_ts: Option<i64>,
    age_minutes: i64,
) -> i64 {
    sqlx::query(
        "INSERT INTO signal_outbox (thread_id, text, quote_ts, queued_at)
         VALUES (?, ?, ?, CURRENT_TIMESTAMP - INTERVAL ? MINUTE)",
    )
    .bind(thread)
    .bind(text)
    .bind(quote_ts)
    .bind(age_minutes)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_id() as i64
}

async fn state(pool: &MySqlPool, id: i64) -> (String, Option<i64>, Option<String>) {
    sqlx::query_as("SELECT CAST(state AS CHAR), sent_ts, note FROM signal_outbox WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_outbox_claims_each_fresh_send_once_with_its_quote() {
    let Some((db, pool)) = connect().await else {
        return;
    };
    let peer = "outbox-peer-uuid";
    let thread = ThreadId::Dm(peer.to_owned());
    db.upsert_conversation(&thread).await.unwrap();
    db.insert_message(&said(&thread, peer, 5_000, "shall we?", false))
        .await
        .unwrap();

    let key = thread.to_string();
    let stale = queue(&pool, &key, "stale", None, 10).await;
    let plain = queue(&pool, &key, "plain", None, 0).await;
    let reply = queue(&pool, &key, "yes", Some(5_000), 0).await;
    let orphan = queue(&pool, &key, "to what?", Some(9_999), 0).await;

    // Drained, since earlier runs' rows share the table; ours in queue order.
    let mut claimed = Vec::new();
    while let Some(send) = db.claim_signal_send(60).await.unwrap() {
        if send.thread == thread {
            claimed.push((send.id, send.text, send.quote));
        }
    }
    assert_eq!(
        claimed,
        vec![
            (plain, "plain".to_owned(), None),
            (
                reply,
                "yes".to_owned(),
                Some(SignalQuote {
                    ts: 5_000,
                    author: peer.to_owned(),
                    text: Some("shall we?".to_owned())
                })
            ),
        ]
    );
    assert_eq!(
        state(&pool, plain).await.0,
        "sending",
        "claimed before the call"
    );
    let (s, _, note) = state(&pool, stale).await;
    assert_eq!(s, "failed");
    assert!(note.unwrap().contains("expired"));
    let (s, _, note) = state(&pool, orphan).await;
    assert_eq!(s, "failed", "a quote the archive cannot name is not sent");
    assert!(note.unwrap().contains("not in the archive"));

    db.record_signal_sent(plain, 7_000, true, None)
        .await
        .unwrap();
    db.record_signal_send_failed(plain, "late").await.unwrap();
    assert_eq!(
        state(&pool, plain).await,
        ("sent".to_owned(), Some(7_000), None),
        "a failure cannot overwrite a send"
    );

    assert!(db.abandon_interrupted_signal_sends().await.unwrap() >= 1);
    let (s, sent_ts, note) = state(&pool, reply).await;
    assert_eq!((s.as_str(), sent_ts), ("failed", None));
    assert!(note.unwrap().contains("may or may not"));
}

#[tokio::test]
async fn a_message_sent_here_is_marked_and_its_sender_is_this_account() {
    let Some((db, pool)) = connect().await else {
        return;
    };
    let me = "outbox-own-uuid";
    let thread = ThreadId::Dm("outbox-other-peer".to_owned());
    db.upsert_conversation(&thread).await.unwrap();
    db.insert_message(&said(&thread, me, 6_000, "from the phone", true))
        .await
        .unwrap();
    assert_eq!(db.own_uuid().await.unwrap().as_deref(), Some(me));

    let id = db
        .insert_message(&said(&thread, me, 6_500, "from the app", true))
        .await
        .unwrap()
        .unwrap();
    db.mark_sent_here(id).await.unwrap();
    let marks: Vec<(Option<String>, bool)> = sqlx::query_as(
        "SELECT body, sent_here FROM messages WHERE thread_id = ? ORDER BY server_ts",
    )
    .bind(thread.to_string())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        marks,
        vec![
            (Some("from the phone".to_owned()), false),
            (Some("from the app".to_owned()), true)
        ]
    );
}
