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
