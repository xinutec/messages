//! Hanging held pictures onto the messages that linked them.
//!
//! No fixture: the join reads links out of the message bodies it is handed and
//! consults one table, so the test hands it messages directly rather than
//! seeding a conversation to reach the same three lines.

use messages::archive::{
    Message, MessageKind, attach_link_images, link_image_state, request_link_image,
};
use messages::link_fetch::url_hash;
use sqlx::MySqlPool;
use sqlx::mysql::MySqlPoolOptions;
use url::Url;

const HELD: &str = "https://cloud.example.org/nc/s/HELD";
const REFUSED: &str = "https://example.com/not-a-picture";

async fn pool() -> Option<MySqlPool> {
    let url = std::env::var("MESSAGES_TEST_DATABASE_URL").ok()?;
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
        // ⚠ Idempotent, because these tests run in PARALLEL against one database
        // and each one seeds. A delete-then-insert setup raced and three of four
        // died on the primary key — the rows are identical every time, so saying
        // so is both the fix and the truth.
        sqlx::query(
            "INSERT INTO link_images (url_hash, url, state, content_type, size_bytes, stored_name, wanted_at, fetched_at)
             VALUES (?, ?, ?, ?, 10, ?, NOW(), NOW())
             ON DUPLICATE KEY UPDATE state = VALUES(state), content_type = VALUES(content_type),
                                     stored_name = VALUES(stored_name)",
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
        link_images: Vec::new(),
        link_offers: Vec::new(),
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
    // ⚠ The row EXISTS — it is a decision, not an absence. Joining on the row
    // rather than on its state would inline a 404 page as a picture.
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
    // The HIDING is the template's job and has its own test there. The API
    // reporting it is what lets a reveal show the picture without a refetch —
    // and what would make a careless template leak it, which is why the gate
    // has a case on both sides of that line.
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
async fn asking_for_an_offered_link_queues_it() {
    let Some(pool) = pool().await else { return };
    let hash = offered_row(&pool).await;
    assert!(request_link_image(&pool, &hash).await.unwrap());
    let (state, _) = link_image_state(&pool, &hash).await.unwrap().unwrap();
    assert_eq!(state, "wanted");
}

#[tokio::test]
async fn asking_for_a_link_nobody_offered_does_nothing() {
    // ⚠ The refusal that keeps the tap from being a fetch-anything endpoint. A
    // hash the archive never produced has no row, so there is nothing to promote
    // and no way for a request to introduce a URL of its own.
    let Some(pool) = pool().await else { return };
    let invented = "f".repeat(64);
    assert!(!request_link_image(&pool, &invented).await.unwrap());
    assert!(link_image_state(&pool, &invented).await.unwrap().is_none());
}

#[tokio::test]
async fn asking_for_a_link_already_decided_does_not_re_queue_it() {
    // A decided link needs no asking, and re-queueing one would send us back to
    // a stranger's server for an answer we already have.
    let Some(pool) = pool().await else { return };
    let hash = url_hash(&Url::parse(REFUSED).unwrap());
    assert!(!request_link_image(&pool, &hash).await.unwrap());
    let (state, _) = link_image_state(&pool, &hash).await.unwrap().unwrap();
    assert_eq!(state, "not_image", "left exactly as it was");
}
