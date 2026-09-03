//! The signature on the session cookie — what makes it unforgeable.
//!
//! ⚠ A session cookie is `<value>.<hex hmac_sha256(value)>`, and this pair is
//! all that stands between "I am logged in" and "I say I am". Every route hangs
//! off the `AuthUser` extractor, which hangs off `verify_value`. It had no test
//! until 2026-09-03, alongside the allow-list it works with.
//!
//! Rejection is the property, not the round trip: a forged, tampered or
//! foreign-signed cookie must fail, and each of those is a separate way in.

use messages::session::{sign_value, verify_value};

const SECRET: &str = "a test secret, not a real one";

#[test]
fn a_cookie_this_secret_signed_verifies_back_to_its_value() {
    let signed = sign_value(SECRET, "abc123");
    assert_eq!(verify_value(SECRET, &signed).as_deref(), Some("abc123"));
}

#[test]
fn a_tampered_value_is_rejected() {
    let signed = sign_value(SECRET, "session-one");
    // Keep the signature, swap the value it vouches for — the forgery a bare
    // `<id>.<anything>` cookie would be.
    let forged = signed.replacen("session-one", "session-two", 1);
    assert_ne!(forged, signed);
    assert_eq!(verify_value(SECRET, &forged), None);
}

#[test]
fn a_tampered_signature_is_rejected() {
    let signed = sign_value(SECRET, "abc123");
    let (value, sig) = signed.rsplit_once('.').unwrap();
    // Flip one hex digit.
    let flipped = match sig.strip_prefix('0') {
        Some(rest) => format!("1{rest}"),
        None => format!("0{}", &sig[1..]),
    };
    assert_eq!(verify_value(SECRET, &format!("{value}.{flipped}")), None);
    // And a truncated one, which a length-only check would wave through.
    assert_eq!(
        verify_value(SECRET, &format!("{value}.{}", &sig[..sig.len() - 2])),
        None
    );
}

#[test]
fn another_secret_cannot_sign_a_cookie_this_one_accepts() {
    let theirs = sign_value("a different secret entirely", "abc123");
    assert_eq!(verify_value(SECRET, &theirs), None);
}

#[test]
fn malformed_cookies_are_rejected_rather_than_panicking() {
    for bad in [
        "",
        "no-dot-at-all",
        "value.",
        ".sig",
        "value.zzzz",
        "value.abc",
    ] {
        assert_eq!(verify_value(SECRET, bad), None, "{bad:?} was accepted");
    }
}

/// ⚠ **`rfind`, NOT `find` — and a `pending_login` payload is why.** That cookie
/// signs `"{timestamp}|{nonce}|{return_to}"`, where `return_to` is a URL path
/// and may contain a dot. Splitting on the FIRST dot would cut the value in the
/// wrong place and reject a login that is perfectly valid — silently, as a
/// failed sign-in rather than an error. No conversation id contains a dot today
/// (checked 2026-09-03: none in either table), so this is a property that would
/// break the first time one did.
#[test]
fn a_value_containing_dots_still_verifies() {
    let payload = "1788356954|abc123nonce|/conversation/gchat/space.a.b";
    let signed = sign_value(SECRET, payload);
    assert_eq!(verify_value(SECRET, &signed).as_deref(), Some(payload));
}
