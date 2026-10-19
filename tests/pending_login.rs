//! The login-in-progress cookie. Nextcloud's Login Flow can return an EMPTY
//! `state`; that callback is never trusted, only retried once, and everything
//! that is not a login we started is refused.

use chrono::{Duration, Utc};
use messages::pending_login::{Callback, PendingLogin, accept, issue, retry, ttl};

/// The pending login, if the callback may exchange its code.
fn accepted(c: Option<Callback>) -> Option<PendingLogin> {
    match c {
        Some(Callback::Accept(p)) => Some(p),
        _ => None,
    }
}

const SECRET: &str = "test-session-secret-0123456789abcdef";

/// Whole seconds: the cookie stores the expiry as a unix timestamp, so a `now`
/// carrying nanoseconds would put the boundary half a tick out.
fn now() -> chrono::DateTime<Utc> {
    chrono::DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("in range")
}

#[test]
fn a_login_completes_when_nextcloud_echoes_the_state() {
    let now = now();
    let (nonce, cookie) = issue(SECRET, Some("/".into()), now);
    let pending = accepted(accept(SECRET, Some(&cookie), Some(&nonce), now)).expect("accepted");
    assert_eq!(pending.return_to.as_deref(), Some("/"));
}

#[test]
fn a_swallowed_state_is_retried_never_trusted() {
    // NC redirects to `/auth/callback?state=&code=…` when the browser had no NC
    // session. Accepting the code then is a login CSRF: anyone's code would do.
    let now = now();
    let (_nonce, cookie) = issue(SECRET, Some("/c/irc/7".into()), now);
    for state in [Some(""), None] {
        match accept(SECRET, Some(&cookie), state, now) {
            Some(Callback::Retry(p)) => assert_eq!(p.return_to.as_deref(), Some("/c/irc/7")),
            other => panic!("an empty state must be retried, not {other:?}"),
        }
    }
}

#[test]
fn the_retry_completes_with_its_state_and_a_second_miss_is_refused() {
    let now = now();
    let (_nonce, cookie) = issue(SECRET, Some("/c/irc/7".into()), now);
    let Some(Callback::Retry(first)) = accept(SECRET, Some(&cookie), None, now) else {
        panic!("first miss retries");
    };
    let (nonce, second) = retry(SECRET, &first, now);
    let done = accepted(accept(SECRET, Some(&second), Some(&nonce), now)).expect("accepted");
    assert_eq!(done.return_to.as_deref(), Some("/c/irc/7"));
    assert!(accept(SECRET, Some(&second), Some(""), now).is_none());
    assert!(accept(SECRET, Some(&second), None, now).is_none());
}

#[test]
fn a_state_that_does_not_match_is_refused() {
    let now = now();
    let (_nonce, cookie) = issue(SECRET, None, now);
    let (other, _) = issue(SECRET, None, now);
    assert!(accept(SECRET, Some(&cookie), Some(&other), now).is_none());
}

#[test]
fn a_callback_with_no_cookie_is_refused() {
    let now = now();
    let (nonce, _cookie) = issue(SECRET, None, now);
    assert!(accept(SECRET, None, Some(&nonce), now).is_none());
}

#[test]
fn a_forged_or_re_signed_cookie_is_refused() {
    let now = now();
    let (nonce, cookie) = issue(SECRET, None, now);
    // Signed with someone else's key.
    assert!(
        accept(
            SECRET,
            Some(&issue("other-secret", None, now).1),
            Some(&nonce),
            now
        )
        .is_none()
    );
    // Payload edited under the original signature.
    let tampered = cookie.replacen(&nonce, &"f".repeat(nonce.len()), 1);
    assert!(accept(SECRET, Some(&tampered), None, now).is_none());
    assert!(accept(SECRET, Some("not-a-cookie"), None, now).is_none());
}

#[test]
fn a_login_left_open_too_long_is_refused() {
    let now = now();
    let (nonce, cookie) = issue(SECRET, None, now);
    let just_inside = now + ttl();
    let past = just_inside + Duration::seconds(1);
    assert!(accept(SECRET, Some(&cookie), Some(&nonce), just_inside).is_some());
    assert!(accept(SECRET, Some(&cookie), Some(&nonce), past).is_none());
}

#[test]
fn a_return_to_containing_the_field_separator_survives() {
    let now = now();
    let (nonce, cookie) = issue(SECRET, Some("/x?q=a|b".into()), now);
    let pending = accepted(accept(SECRET, Some(&cookie), Some(&nonce), now)).expect("accepted");
    assert_eq!(pending.return_to.as_deref(), Some("/x?q=a|b"));
}

#[test]
fn no_return_to_round_trips_as_none() {
    let now = now();
    let (nonce, cookie) = issue(SECRET, None, now);
    assert_eq!(
        accepted(accept(SECRET, Some(&cookie), Some(&nonce), now))
            .unwrap()
            .return_to,
        None
    );
}
