//! Migrations run one process at a time: every binary migrates at startup, and
//! a deploy starts them together.
//!
//! Skips when `SIGNAL_TEST_DATABASE_URL` is unset, and refuses to skip in CI.

use std::time::Duration;

use signal_archiver::db::Db;
use sqlx::mysql::MySqlPoolOptions;

#[tokio::test]
async fn a_second_process_waits_for_the_migration_lock_and_leaves_it_free() {
    let Ok(url) = std::env::var("SIGNAL_TEST_DATABASE_URL") else {
        // A skip passes, so CI must not skip.
        assert!(
            std::env::var("CI").is_err(),
            "SIGNAL_TEST_DATABASE_URL is unset in CI: the migration lock would \
             ship unverified"
        );
        eprintln!("SIGNAL_TEST_DATABASE_URL unset — skipping");
        return;
    };
    // Bring the schema up first, so the connect under test has nothing to apply
    // and its time is all waiting.
    Db::connect(&url).await.expect("migrations apply");

    let holder = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connect");
    let mut held = holder.acquire().await.expect("a connection");
    let got: Option<i64> = sqlx::query_scalar("SELECT GET_LOCK('signal_migrate', 5)")
        .fetch_one(&mut *held)
        .await
        .expect("take the lock");
    assert_eq!(got, Some(1), "another test holds the lock");

    let migrating = tokio::spawn({
        let url = url.clone();
        async move { Db::connect(&url).await.map(|_| ()) }
    });
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        !migrating.is_finished(),
        "a process migrated while another held the lock"
    );

    sqlx::query("SELECT RELEASE_LOCK('signal_migrate')")
        .execute(&mut *held)
        .await
        .expect("release");
    tokio::time::timeout(Duration::from_secs(20), migrating)
        .await
        .expect("it goes ahead once the lock is free")
        .expect("no panic")
        .expect("migrations apply");

    let free: Option<i64> = sqlx::query_scalar("SELECT IS_FREE_LOCK('signal_migrate')")
        .fetch_one(&mut *held)
        .await
        .expect("ask");
    assert_eq!(free, Some(1), "the migrating process released what it took");
}
