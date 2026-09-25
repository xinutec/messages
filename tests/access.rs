//! The allow-list: who may use this app at all.
//!
//! `Config::is_allowed`, called on the OAuth callback, is what the security
//! model rests on: the ingress also answers on the public IP. Every way of
//! configuring nobody must reject everybody.

use messages::config::{Config, parse_allowed_users};

#[path = "support/config.rs"]
mod support;

fn cfg(allowed: &[&str]) -> Config {
    Config {
        allowed_users: allowed.iter().map(|s| (*s).to_string()).collect(),
        ..support::config()
    }
}

#[test]
fn a_listed_user_is_allowed_and_nobody_else_is() {
    let c = cfg(&["pippijn"]);
    assert!(c.is_allowed("pippijn"));
    assert!(!c.is_allowed("simon"));
    // Not a prefix match.
    assert!(!c.is_allowed("pippijn2"));
    assert!(!c.is_allowed("ippijn"));
}

#[test]
fn an_empty_list_admits_nobody() {
    let c = cfg(&[]);
    assert!(!c.is_allowed("pippijn"));
    assert!(!c.is_allowed(""));
}

#[test]
fn an_empty_user_id_is_never_allowed() {
    // An empty caller id matches nothing.
    assert!(!cfg(&["pippijn"]).is_allowed(""));
}

#[test]
fn the_parser_drops_the_empty_entries_that_would_admit_an_empty_id() {
    // A blank variable or a stray comma leaves no empty entry.
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
    let c = cfg(&[]);
    for raw in ["", "   ", ",,", " , "] {
        assert!(
            parse_allowed_users(raw).is_empty(),
            "{raw:?} parsed to something"
        );
    }
    assert!(!c.is_allowed("anyone"));
}
