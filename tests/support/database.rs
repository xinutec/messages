//! The throwaway database the archive tests run against.

/// `MESSAGES_TEST_DATABASE_URL`, or `None` to skip. A skip passes, so CI must
/// not skip.
pub fn database_url() -> Option<String> {
    let Ok(url) = std::env::var("MESSAGES_TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "MESSAGES_TEST_DATABASE_URL is unset in CI: these tests would skip \
             and ship unverified"
        );
        eprintln!("MESSAGES_TEST_DATABASE_URL unset — skipping");
        return None;
    };
    Some(url)
}
