//! Where a login may send the browser afterwards: only somewhere on this site.

use messages::routes::auth::validate_return_to;

#[test]
fn an_internal_path_is_kept() {
    assert_eq!(validate_return_to(Some("/c/irc/7")), "/c/irc/7");
    assert_eq!(validate_return_to(Some("/")), "/");
}

/// `//host` and `/\host` both name another site; browsers read the backslash as
/// a slash.
#[test]
fn another_site_is_refused_however_it_is_spelled() {
    for elsewhere in [
        "//evil.example/x",
        "/\\evil.example/x",
        "https://evil.example/x",
        "evil.example",
        "",
    ] {
        assert_eq!(
            validate_return_to(Some(elsewhere)),
            "/",
            "{elsewhere:?} was allowed"
        );
    }
    assert_eq!(validate_return_to(None), "/");
}

/// A browser's URL parser drops tabs and newlines and reads `\` as `/`, so
/// `/\t/host` lands on `//host`, another site. Each attempt must land here.
#[test]
fn every_accepted_path_resolves_to_this_site() {
    let site = url::Url::parse("https://messages.example/").unwrap();
    for attempt in [
        "/\t/evil.example",
        "/\n/evil.example",
        "/\r/evil.example",
        "/\t\\evil.example",
        "/ /evil.example",
        "/c/irc/7",
    ] {
        let dest = validate_return_to(Some(attempt));
        let landed = site.join(&dest).unwrap();
        assert_eq!(
            landed.host_str(),
            Some("messages.example"),
            "{attempt:?} was let through as {dest:?}"
        );
    }
}
