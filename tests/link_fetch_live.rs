//! ⚠ **NETWORK, AND THEREFORE `#[ignore]`d.** These reach a real file cloud on
//! the open internet. They are not in the gate and must never be: CI has no
//! egress, the pod has none either, and a suite that runs on every commit is not
//! something to point at a friend's server.
//!
//! Run by hand when the resolver changes:
//!
//!     nix run ../dev-lint#with-test-db -- messages_test --user messages \
//!       --password messages --port 3318 --url-env MESSAGES_TEST_DATABASE_URL -- \
//!       cargo test --test link_fetch_live -- --ignored --nocapture
//!
//! The URL is passed in rather than committed, because a share link is a
//! capability and this repository is public:
//!
//!     LINK_FETCH_LIVE_URL=https://…/s/TOKEN

use messages::link_fetch::{Limits, Outcome, resolve_one, url_hash};
use sqlx::mysql::MySqlPoolOptions;
use url::Url;

#[tokio::test]
#[ignore = "reaches the open internet; run by hand"]
async fn a_share_link_becomes_bytes_on_disk() {
    let (Ok(db), Ok(link)) = (
        std::env::var("MESSAGES_TEST_DATABASE_URL"),
        std::env::var("LINK_FETCH_LIVE_URL"),
    ) else {
        eprintln!("set MESSAGES_TEST_DATABASE_URL and LINK_FETCH_LIVE_URL");
        return;
    };
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect(&db)
        .await
        .unwrap();
    messages::db::ensure_schema(&pool).await.unwrap();

    let dir = std::env::temp_dir().join(format!("link-images-{}", std::process::id()));
    let client = reqwest::Client::builder()
        .user_agent("xinutec-messages/1.0 (link preview; one fetch per link, ever)")
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .unwrap();
    let url = Url::parse(&link).unwrap();

    let outcome = resolve_one(&pool, &client, &dir, &url, Limits::default())
        .await
        .unwrap();
    assert_eq!(
        outcome,
        Outcome::Ok,
        "a share page that advertises a picture must resolve"
    );

    let (state, content_type, size, name): (String, Option<String>, Option<i64>, Option<String>) =
        sqlx::query_as("SELECT state, content_type, size_bytes, stored_name FROM link_images WHERE url_hash = ?")
            .bind(url_hash(&url))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "ok");
    assert!(
        content_type
            .as_deref()
            .unwrap_or_default()
            .starts_with("image/"),
        "{content_type:?}"
    );
    let bytes = std::fs::read(dir.join(name.unwrap())).unwrap();
    assert_eq!(
        bytes.len() as i64,
        size.unwrap(),
        "stored size is what landed on disk"
    );
    // Real pixels, not an error page with an image content type.
    assert!(
        bytes.len() > 10_000,
        "suspiciously small for a photo: {} bytes",
        bytes.len()
    );
    eprintln!(
        "resolved {} → {} bytes, {}",
        url.as_str().rsplit('/').next().unwrap(),
        bytes.len(),
        content_type.unwrap()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "reaches the open internet; run by hand"]
async fn a_page_that_is_not_a_file_cloud_is_remembered_as_not_one() {
    let Ok(db) = std::env::var("MESSAGES_TEST_DATABASE_URL") else {
        return;
    };
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect(&db)
        .await
        .unwrap();
    messages::db::ensure_schema(&pool).await.unwrap();
    let dir = std::env::temp_dir().join(format!("link-images-neg-{}", std::process::id()));
    let client = reqwest::Client::builder()
        .user_agent("xinutec-messages/1.0")
        .build()
        .unwrap();
    // example.com serves HTML, sets no file-cloud cookie, advertises no picture.
    let url = Url::parse("https://example.com/").unwrap();
    assert_eq!(
        resolve_one(&pool, &client, &dir, &url, Limits::default())
            .await
            .unwrap(),
        Outcome::NotImage
    );
    let _ = std::fs::remove_dir_all(&dir);
}
