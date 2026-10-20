//! Verdicts an earlier version wrote down come back as offers when the schema is
//! brought up to date. Its own binary: it widens the shared table's enum for a
//! moment, which the other tests, running in parallel within a binary, must
//! not see.

use messages::link_fetch::url_hash;
use sqlx::MySqlPool;
use sqlx::mysql::MySqlPoolOptions;
use url::Url;

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
    Some(pool)
}

#[tokio::test]
async fn a_written_down_no_becomes_an_offer_again() {
    let Some(pool) = pool().await else { return };
    // The table as the earlier version left it.
    sqlx::query(
        "ALTER TABLE link_images
           MODIFY COLUMN state ENUM('offered','ok','not_image','failed') NOT NULL,
           ADD COLUMN IF NOT EXISTS note VARCHAR(255) NULL,
           ADD COLUMN IF NOT EXISTS decided_by INT NULL",
    )
    .execute(&pool)
    .await
    .unwrap();
    let urls = [
        "https://example.com/judged-no",
        "https://example.com/was-down",
    ];
    for (url, state) in urls.iter().zip(["not_image", "failed"]) {
        sqlx::query(
            "REPLACE INTO link_images (url_hash, url, state, note, decided_by, wanted_at, fetched_at)
             VALUES (?, ?, ?, 'why', 2, NOW(), NOW())",
        )
        .bind(url_hash(&Url::parse(url).unwrap()))
        .bind(*url)
        .bind(state)
        .execute(&pool)
        .await
        .unwrap();
    }

    messages::db::ensure_schema(&pool).await.unwrap();

    for url in urls {
        let (state, fetched): (String, Option<chrono::NaiveDateTime>) =
            sqlx::query_as("SELECT state, fetched_at FROM link_images WHERE url_hash = ?")
                .bind(url_hash(&Url::parse(url).unwrap()))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "offered", "{url}");
        assert!(fetched.is_none(), "{url}: nothing was fetched that is held");
    }
    let shape: Vec<String> = sqlx::query_scalar(
        "SELECT CONCAT(column_name, ' ', column_type) FROM information_schema.columns
          WHERE table_schema = DATABASE() AND table_name = 'link_images'
            AND column_name IN ('state', 'note', 'decided_by')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(shape, ["state enum('offered','ok')"]);
}
