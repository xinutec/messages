//! Hanging held pictures onto the messages that linked them.
//!
//! No fixture: the join reads links from the bodies it is handed, so the tests
//! hand it messages directly.

use messages::archive::links::{attach, offered_url};
use messages::archive::{Message, MessageKind};
use messages::link_fetch::{Outcome, resolve_one, url_hash};
use sqlx::MySqlPool;
use sqlx::mysql::MySqlPoolOptions;
use url::Url;

const HELD: &str = "https://cloud.example.org/nc/s/HELD";

#[path = "support/database.rs"]
mod database;

async fn pool() -> Option<MySqlPool> {
    let url = database::database_url()?;
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    messages::db::ensure_schema(&pool).await.unwrap();

    // Idempotent: these tests seed in parallel.
    sqlx::query(
        "INSERT INTO link_images (url_hash, url, state, content_type, size_bytes, stored_name, wanted_at, fetched_at)
         VALUES (?, ?, 'ok', 'image/jpeg', 10, 'held.jpg', NOW(), NOW())
         ON DUPLICATE KEY UPDATE state = VALUES(state), content_type = VALUES(content_type),
                                 stored_name = VALUES(stored_name)",
    )
    .bind(url_hash(&Url::parse(HELD).unwrap()))
    .bind(HELD)
    .execute(&pool)
    .await
    .unwrap();
    Some(pool)
}

fn msg(body: &str) -> Message {
    Message {
        id: "1".into(),
        ts: 0,
        sender: "s".into(),
        sender_key: "s".into(),
        is_outgoing: false,
        kind: MessageKind::Message,
        body: Some(body.into()),
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
    }
}

#[tokio::test]
async fn a_link_we_hold_a_picture_for_is_hung_on_its_message() {
    let Some(pool) = pool().await else { return };
    let mut msgs = [msg(&format!("look at {HELD} then"))];
    attach(&pool, &mut msgs).await.unwrap();
    assert_eq!(msgs[0].link_images.len(), 1);
    assert_eq!(
        msgs[0].link_images[0].url, HELD,
        "the link as typed, so the UI can link out to it"
    );
    assert_eq!(msgs[0].link_images[0].content_type, "image/jpeg");
}

#[tokio::test]
async fn a_message_with_no_link_costs_no_query() {
    let Some(pool) = pool().await else { return };
    let mut msgs = [msg("no links at all")];
    attach(&pool, &mut msgs).await.unwrap();
    assert!(msgs[0].link_images.is_empty());
}

#[tokio::test]
async fn a_deleted_message_still_reports_what_we_hold() {
    // The template hides it; the API reports it, so a reveal needs no refetch.
    let Some(pool) = pool().await else { return };
    let mut msgs = [Message {
        deleted: true,
        ..msg(&format!("gone, but {HELD}"))
    }];
    attach(&pool, &mut msgs).await.unwrap();
    assert_eq!(msgs[0].link_images.len(), 1);
}

// ---- asking for one -------------------------------------------------------

const OFFERED: &str = "https://cloud.example.org/nc/s/OFFERED";

async fn offered_row(pool: &MySqlPool) -> String {
    let url = Url::parse(OFFERED).unwrap();
    let hash = url_hash(&url);
    sqlx::query(
        "INSERT INTO link_images (url_hash, url, state, wanted_at) VALUES (?, ?, 'offered', NOW())
         ON DUPLICATE KEY UPDATE state = 'offered'",
    )
    .bind(&hash)
    .bind(OFFERED)
    .execute(pool)
    .await
    .unwrap();
    hash
}

#[tokio::test]
async fn an_offered_link_resolves_to_the_address_the_archive_gave_it() {
    let Some(pool) = pool().await else { return };
    let hash = offered_row(&pool).await;
    assert_eq!(
        offered_url(&pool, &hash).await.unwrap().as_deref(),
        Some(OFFERED)
    );
}

#[tokio::test]
async fn a_hash_nobody_offered_resolves_to_no_address() {
    // An invented hash names nothing: a caller cannot introduce a URL.
    let Some(pool) = pool().await else { return };
    assert!(offered_url(&pool, &"f".repeat(64)).await.unwrap().is_none());
}

// ---- offering ------------------------------------------------------------

/// The `link_images.state` a link's row holds.
async fn stored_state(pool: &MySqlPool, hash: &str) -> String {
    sqlx::query_scalar("SELECT state FROM link_images WHERE url_hash = ?")
        .bind(hash)
        .fetch_one(pool)
        .await
        .unwrap()
}

const FRESH: &str = "https://cloud.example.org/nc/s/FRESH";

#[tokio::test]
async fn serving_a_message_offers_its_undecided_links() {
    // Serving a page offers the link: a row with the archive's URL, nothing
    // fetched, and an offer on the message.
    let Some(pool) = pool().await else { return };
    let hash = url_hash(&Url::parse(FRESH).unwrap());
    sqlx::query("DELETE FROM link_images WHERE url_hash = ?")
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();

    let mut msgs = [msg(&format!("look at {FRESH} then"))];
    attach(&pool, &mut msgs).await.unwrap();

    assert!(msgs[0].link_images.is_empty(), "nothing is held yet");
    assert_eq!(msgs[0].link_offers.len(), 1, "the reader is offered it");
    assert_eq!(msgs[0].link_offers[0].url, FRESH);

    assert_eq!(
        stored_state(&pool, &hash).await,
        "offered",
        "registered, not queued"
    );
}

#[tokio::test]
async fn offering_a_held_link_again_leaves_it_held() {
    // Serving a page again changes nothing known about its links.
    let Some(pool) = pool().await else { return };
    let hash = url_hash(&Url::parse(HELD).unwrap());
    let mut msgs = [msg(&format!("and {HELD} here"))];
    attach(&pool, &mut msgs).await.unwrap();
    assert_eq!(stored_state(&pool, &hash).await, "ok");
    assert!(msgs[0].link_offers.is_empty(), "a held link is not offered");
}

// ---- what an answer leaves behind -------------------------------------------

/// A stand-in for the link-fetch pod, answering every `/fetch` the same way.
async fn fetcher(status: u16, content_type: &'static str, body: &'static [u8]) -> String {
    use axum::http::{StatusCode, header};
    let app = axum::Router::new().route(
        "/fetch",
        axum::routing::post(move || async move {
            (
                StatusCode::from_u16(status).unwrap(),
                [(header::CONTENT_TYPE, content_type)],
                body,
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// Offer `url` afresh, then ask a fetcher that answers as given.
async fn ask(
    pool: &MySqlPool,
    url: &str,
    status: u16,
    content_type: &'static str,
    body: &'static [u8],
) -> (Outcome, String) {
    let parsed = Url::parse(url).unwrap();
    let hash = url_hash(&parsed);
    sqlx::query("DELETE FROM link_images WHERE url_hash = ?")
        .bind(&hash)
        .execute(pool)
        .await
        .unwrap();
    attach(pool, &mut [msg(url)]).await.unwrap();
    let service = fetcher(status, content_type, body).await;
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("link-images");
    let (outcome, _) = resolve_one(pool, &reqwest::Client::new(), &service, &dir, &parsed)
        .await
        .unwrap();
    (outcome, hash)
}

/// "Not a picture" and "could not reach it" are said to the reader once and
/// never written down: the rule for what counts as a picture changes, and so
/// does the page, so the link is offered again on the next read.
#[tokio::test]
async fn a_no_is_said_once_and_never_written_down() {
    let Some(pool) = pool().await else { return };
    for (url, status, said) in [
        (
            "https://example.com/no-picture-here",
            204,
            Outcome::NotImage,
        ),
        ("https://example.com/server-down", 502, Outcome::Failed),
    ] {
        let (outcome, hash) = ask(&pool, url, status, "text/plain", b"").await;
        assert_eq!(outcome, said, "{url}");
        assert_eq!(stored_state(&pool, &hash).await, "offered", "{url}");
        assert_eq!(
            offered_url(&pool, &hash).await.unwrap().as_deref(),
            Some(url),
            "{url} can be asked again"
        );
        let mut again = [msg(url)];
        attach(&pool, &mut again).await.unwrap();
        assert_eq!(again[0].link_offers.len(), 1, "{url} is offered again");
    }
}

#[tokio::test]
async fn a_picture_is_held() {
    let Some(pool) = pool().await else { return };
    let url = "https://example.com/a-picture.png";
    let (outcome, hash) = ask(&pool, url, 200, "image/png", b"\x89PNG").await;
    assert_eq!(outcome, Outcome::Ok);
    assert_eq!(stored_state(&pool, &hash).await, "ok");
    assert!(
        offered_url(&pool, &hash).await.unwrap().is_none(),
        "held, so not asked for"
    );
}
