//! The allow-list: who may use this app at all.
//!
//! ⚠ **This is the gate the security model actually rests on.** README's layer 2
//! — VPN-only by DNS — is admitted obscurity there, because the isis ingress
//! answers on the public IP too. So what stands between an authenticated
//! stranger and a private message archive is `Config::is_allowed`, called from
//! `routes/auth.rs` on the OAuth callback. It had no test of any kind until
//! 2026-09-03; it reads correctly, and nothing would have said so if it stopped.
//!
//! Fail-closed is the property under test, not merely "the happy path works":
//! every way of ending up with nobody on the list must reject everybody.

use messages::config::{Config, parse_allowed_users};
use sqlx::mysql::MySqlConnectOptions;

/// A Config that is nonsense everywhere except the field under test — none of
/// the rest is read by `is_allowed`, and spelling them out keeps it obvious that
/// the answer cannot be coming from anywhere else.
fn cfg(allowed: &[&str]) -> Config {
    Config {
        db_options: MySqlConnectOptions::new(),
        session_secret: String::new(),
        bind_addr: String::new(),
        nc_base_url: String::new(),
        nc_client_id: String::new(),
        nc_client_secret: String::new(),
        nc_redirect_uri: String::new(),
        allowed_users: allowed.iter().map(|s| (*s).to_string()).collect(),
        static_dir: None,
        attachments_dir: String::new(),
        irc_send: None,
    }
}

#[test]
fn a_listed_user_is_allowed_and_nobody_else_is() {
    let c = cfg(&["pippijn"]);
    assert!(c.is_allowed("pippijn"));
    assert!(!c.is_allowed("simon"));
    // Not a prefix or substring match: `pippijn2` is a different account.
    assert!(!c.is_allowed("pippijn2"));
    assert!(!c.is_allowed("ippijn"));
}

#[test]
fn an_empty_list_admits_nobody() {
    // ⚠ The fail-closed case. `any()` over an empty list is false, which is the
    // behaviour we want and the one an "obvious" refactor to `is_empty() ||` —
    // read as "no list configured means no restriction" — would invert.
    let c = cfg(&[]);
    assert!(!c.is_allowed("pippijn"));
    assert!(!c.is_allowed(""));
}

#[test]
fn an_empty_user_id_is_never_allowed() {
    // A caller handing us "" must not match, whatever is on the list. This is
    // what the parser's empty-entry filter protects, from the other side.
    assert!(!cfg(&["pippijn"]).is_allowed(""));
}

#[test]
fn the_parser_drops_the_empty_entries_that_would_admit_an_empty_id() {
    // ⚠ ALLOWED_USERS unset-but-present, or with a stray comma, is how a list
    // acquires an empty entry — and an empty entry plus an empty user id is an
    // open door. Trimmed and filtered, so neither survives.
    assert!(parse_allowed_users("").is_empty());
    assert!(parse_allowed_users("   ").is_empty());
    assert!(parse_allowed_users(",,").is_empty());
    assert_eq!(parse_allowed_users("pippijn"), vec!["pippijn"]);
    assert_eq!(
        parse_allowed_users(" pippijn , simon "),
        vec!["pippijn", "simon"]
    );
    assert_eq!(
        parse_allowed_users("pippijn,,simon,"),
        vec!["pippijn", "simon"]
    );
}

#[test]
fn a_list_that_parsed_to_nothing_still_admits_nobody() {
    // The two halves together: a misconfigured ALLOWED_USERS cannot open the app
    // up, which is the whole claim.
    let c = cfg(&[]);
    for raw in ["", "   ", ",,", " , "] {
        assert!(
            parse_allowed_users(raw).is_empty(),
            "{raw:?} parsed to something"
        );
    }
    assert!(!c.is_allowed("anyone"));
}
