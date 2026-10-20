//! An edited Signal message's versions all point at the original, however the
//! edits arrive: Signal can address an edit to the edit before it, and the
//! viewer gathers a message's versions by the original's timestamp alone.
//!
//! Skips when `SIGNAL_TEST_DATABASE_URL` is unset, and refuses to skip in CI.

use signal_archiver::db::Db;
use signal_archiver::parse::{Message, ThreadId};
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};

async fn connect() -> Option<(Db, MySqlPool)> {
    let Ok(url) = std::env::var("SIGNAL_TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "SIGNAL_TEST_DATABASE_URL is unset in CI: the edit writes would skip \
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

fn original(sender: &str, ts: i64) -> Message {
    Message {
        thread_id: ThreadId::Dm(sender.to_owned()),
        sender: sender.to_owned(),
        server_ts: ts,
        server_received_ts: None,
        server_delivered_ts: None,
        expires_in_seconds: None,
        body: Some("first words".to_owned()),
        quote_target_ts: None,
        quote_author: None,
        quote_text: None,
        is_outgoing: false,
        attachments: Vec::new(),
        styles: Vec::new(),
        mentions: Vec::new(),
        previews: Vec::new(),
    }
}

#[tokio::test]
async fn an_edit_of_an_edit_is_filed_against_the_original() {
    let Some((db, pool)) = connect().await else {
        return;
    };
    let pid = i64::from(std::process::id());
    let sender = format!("edits-{pid}");
    let thread = ThreadId::Dm(sender.clone());
    let sent = 1_700_000_000_000 + pid * 1_000;
    let (first, second) = (sent + 10, sent + 20);

    db.insert_message(&original(&sender, sent)).await.unwrap();
    db.mark_edited(&sender, sent).await.unwrap();
    db.insert_edit(&thread, &sender, first, Some("second words"), sent, false)
        .await
        .unwrap();
    // Signal names the edit it replaces, not the original.
    db.mark_edited(&sender, first).await.unwrap();
    db.insert_edit(&thread, &sender, second, Some("third words"), first, false)
        .await
        .unwrap();

    let points: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT server_ts, edit_of_ts FROM messages
          WHERE sender_uuid = ? AND edit_of_ts IS NOT NULL ORDER BY server_ts",
    )
    .bind(&sender)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(points, [(first, sent), (second, sent)]);
}
