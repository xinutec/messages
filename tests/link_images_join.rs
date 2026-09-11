//! Hanging held pictures onto the messages that linked them.
//!
//! No fixture: the join reads links out of the message bodies it is handed and
//! consults one table, so the test hands it messages directly rather than
//! seeding a conversation to reach the same three lines.

use messages::archive::{Message, MessageKind, attach_link_images, link_image_state, offered_url};
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
    // ⚠ The refusal the whole tap rests on. A request carries a hash; the address
    // comes from the row serving a page created. An invented hash therefore names
    // nothing, and there is no path by which a caller can introduce a URL to
    // fetch — which is what would turn this into an open proxy with a button.
    let Some(pool) = pool().await else { return };
    assert!(offered_url(&pool, &"f".repeat(64)).await.unwrap().is_none());
}

#[tokio::test]
async fn a_decided_link_is_not_offered_again() {
    // Decided BY THIS READER, so there is nothing to ask for. A verdict from an
    // older reader is a different matter and has its own test in link_image.rs.
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
    // ⚠ THE PATH THAT MAKES A TAP POSSIBLE, and every other test here seeds a
    // link that is already decided — so this one was never exercised until it
    // was written. Serving a page must leave the link offered: a row with the
    // URL from the archive, nothing fetched, and the message carrying the offer
    // so the UI can draw a control.
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
    // A second reading of a conversation must not undo what is known about its
    // links — serving a page is a read with one insert, never an update.
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
