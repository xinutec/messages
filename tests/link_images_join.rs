//! Hanging held pictures onto the messages that linked them.
//!
//! No fixture: the join reads links from the bodies it is handed, so the tests
//! hand it messages directly.

use messages::archive::{Message, MessageKind, attach_link_images, link_image_state, offered_url};
use messages::link_fetch::url_hash;
use sqlx::MySqlPool;
use sqlx::mysql::MySqlPoolOptions;
use url::Url;

const HELD: &str = "https://cloud.example.org/nc/s/HELD";
const REFUSED: &str = "https://example.com/not-a-picture";

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

    for (url, state, ct, name) in [
        (HELD, "ok", Some("image/jpeg"), Some("held.jpg")),
        (REFUSED, "not_image", None, None),
    ] {
        // Idempotent: these tests seed in parallel.
        sqlx::query(
            "INSERT INTO link_images (url_hash, url, state, content_type, size_bytes, stored_name, wanted_at, fetched_at, decided_by)
             VALUES (?, ?, ?, ?, 10, ?, NOW(), NOW(), 2)
             ON DUPLICATE KEY UPDATE state = VALUES(state), content_type = VALUES(content_type),
                                     stored_name = VALUES(stored_name), decided_by = VALUES(decided_by)",
        )
        .bind(url_hash(&Url::parse(url).unwrap()))
        .bind(url)
        .bind(state)
        .bind(ct)
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
    }
    Some(pool)
}

fn msg(body: &str) -> Message {
    Message {
        id: "1".into(),
        ts: 0,
        sender: "s".into(),
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
    attach_link_images(&pool, &mut msgs).await.unwrap();
    assert_eq!(msgs[0].link_images.len(), 1);
    assert_eq!(
        msgs[0].link_images[0].url, HELD,
        "the link as typed, so the UI can link out to it"
    );
    assert_eq!(msgs[0].link_images[0].content_type, "image/jpeg");
}

#[tokio::test]
async fn a_link_we_decided_against_stays_a_link() {
    // The row exists, as a decision. Joining on the row alone would inline a
    // 404 page.
    let Some(pool) = pool().await else { return };
    let mut msgs = [msg(&format!("and {REFUSED} here"))];
    attach_link_images(&pool, &mut msgs).await.unwrap();
    assert!(msgs[0].link_images.is_empty());
}

#[tokio::test]
async fn a_message_with_no_link_costs_no_query() {
    let Some(pool) = pool().await else { return };
    let mut msgs = [msg("no links at all")];
    attach_link_images(&pool, &mut msgs).await.unwrap();
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
    attach_link_images(&pool, &mut msgs).await.unwrap();
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

#[tokio::test]
async fn a_decided_link_is_not_offered_again() {
    // Decided by this reader, so nothing to ask. Older readers: link_image.rs.
    let Some(pool) = pool().await else { return };
    let hash = url_hash(&Url::parse(REFUSED).unwrap());
    assert!(offered_url(&pool, &hash).await.unwrap().is_none());
    let (state, _) = link_image_state(&pool, &hash).await.unwrap().unwrap();
    assert_eq!(state, "not_image", "left exactly as it was");
}

// ---- offering ------------------------------------------------------------

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
    attach_link_images(&pool, &mut msgs).await.unwrap();

    assert!(msgs[0].link_images.is_empty(), "nothing is held yet");
    assert_eq!(msgs[0].link_offers.len(), 1, "the reader is offered it");
    assert_eq!(msgs[0].link_offers[0].url, FRESH);

    let (state, _) = link_image_state(&pool, &hash).await.unwrap().unwrap();
    assert_eq!(state, "offered", "registered, not queued");
}

#[tokio::test]
async fn offering_a_link_twice_leaves_a_decision_alone() {
    // Serving a page again changes nothing known about its links.
    let Some(pool) = pool().await else { return };
    let hash = url_hash(&Url::parse(REFUSED).unwrap());
    let mut msgs = [msg(&format!("and {REFUSED} here"))];
    attach_link_images(&pool, &mut msgs).await.unwrap();
    let (state, _) = link_image_state(&pool, &hash).await.unwrap().unwrap();
    assert_eq!(state, "not_image");
    assert!(
        msgs[0].link_offers.is_empty(),
        "a decided link is not offered"
    );
}
