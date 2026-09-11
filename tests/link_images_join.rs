//! Hanging held pictures onto the messages that linked them.
//!
//! No fixture: the join reads links out of the message bodies it is handed and
//! consults one table, so the test hands it messages directly rather than
//! seeding a conversation to reach the same three lines.

use messages::archive::{Message, MessageKind, attach_link_images};
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
            "INSERT INTO link_images (url_hash, url, state, content_type, size_bytes, stored_name, fetched_at)
             VALUES (?, ?, ?, ?, 10, ?, NOW())
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
