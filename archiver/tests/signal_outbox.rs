//! The Signal outbox: what the ingester claims, what it reads for a quote, and
//! how a send's outcome lands.
//!
//! Skips when `SIGNAL_TEST_DATABASE_URL` is unset, and refuses to skip in CI.

use signal_archiver::db::{Db, SignalQuote};
use signal_archiver::parse::{Message, ThreadId};
use signal_archiver::send::{Sent, Target, deliver};
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// This account. Every test here sends as it: `own_uuid` reads the newest
/// outgoing row, and the tests run side by side.
const ME: &str = "outbox-own-uuid";

/// The outbox is one queue, drained whole by a claim: two tests draining it at
/// once deadlock in InnoDB and take each other's rows. One ingester claims in
/// production; here the draining tests take turns.
static DRAINING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    let _turn = DRAINING.lock().await;
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
    let me = ME;
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

/// signal-cli-rest-api for one request: answers `status` with `body`, and hands
/// back the request body it was sent.
async fn stand_in(
    status: &'static str,
    body: &'static str,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served = tokio::spawn(async move {
        let (mut conn, _) = listener.accept().await.unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        let request = loop {
            let n = conn.read(&mut buf).await.unwrap();
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got).into_owned();
            let Some(split) = text.find("\r\n\r\n") else {
                continue;
            };
            let length = text[..split]
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if got.len() >= split + 4 + length {
                break text[split + 4..split + 4 + length].to_owned();
            }
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        conn.write_all(response.as_bytes()).await.unwrap();
        request
    });
    (base, served)
}

/// A claimed reply goes to `/v2/send` as signal-cli-rest-api 0.101 wants it, and
/// its answer, a list of one response, is archived as this account's message,
/// quoting what it answers and marked as sent here.
#[tokio::test]
async fn a_reply_is_sent_and_archived_from_signals_receipt() {
    let Some((db, pool)) = connect().await else {
        return;
    };
    let _turn = DRAINING.lock().await;
    let peer = "deliver-peer-uuid";
    let thread = ThreadId::Dm(peer.to_owned());
    db.upsert_conversation(&thread).await.unwrap();
    db.insert_message(&said(&thread, peer, 8_000, "about that?", false))
        .await
        .unwrap();
    db.insert_message(&said(&thread, ME, 8_500, "from the phone", true))
        .await
        .unwrap();
    queue(&pool, &thread.to_string(), "yes, that", Some(8_000), 0).await;
    let mut ours = None;
    while let Some(send) = db.claim_signal_send(60).await.unwrap() {
        if send.thread == thread {
            ours = Some(send);
        }
    }
    let send = ours.expect("the reply was claimed");

    let (base, served) = stand_in("201 Created", r#"[{"timestamp":"1793051786342"}]"#).await;
    let http = reqwest::Client::new();
    let to = Target {
        http: &http,
        base: &base,
        number: "+10000000000",
        timeout: std::time::Duration::from_secs(5),
    };
    assert_eq!(
        deliver(&db, &to, &send).await,
        Ok(Sent {
            ts: 1_793_051_786_342,
            archived: true,
            missed: None
        })
    );

    let asked: serde_json::Value = serde_json::from_str(&served.await.unwrap()).unwrap();
    assert_eq!(asked["number"], "+10000000000");
    assert_eq!(asked["recipients"], serde_json::json!([peer]));
    assert_eq!(asked["message"], "yes, that");
    assert_eq!(asked["quote_timestamp"], 8_000);
    assert_eq!(asked["quote_author"], peer);
    assert_eq!(asked["quote_message"], "about that?");

    let row: (String, Option<String>, Option<i64>, bool, bool) = sqlx::query_as(
        "SELECT sender_uuid, body, quote_target_ts, is_outgoing, sent_here
           FROM messages WHERE thread_id = ? AND server_ts = ?",
    )
    .bind(thread.to_string())
    .bind(1_793_051_786_342i64)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            ME.to_owned(),
            Some("yes, that".to_owned()),
            Some(8_000),
            true,
            true
        )
    );
}
