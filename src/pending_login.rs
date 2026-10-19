//! The pending half of a Nextcloud login, carried in a signed cookie.
//!
//! Why not the `state` parameter alone. When the browser holds no Nextcloud
//! session, NC's `oauth2/authorize` does not redirect back to us — it bounces to its
//! own Login Flow, and drops every query parameter on the way:
//!
//! ```text
//! GET …/oauth2/authorize?client_id=…&redirect_uri=…&state=f360a3be…
//!  → 303 …/login/flow?providedRedirectUri=&clientIdentifier=…
//! ```
//!
//! It then returns to the callback with an empty `state`. That callback is never
//! trusted (RFC 6749 §10.12, login CSRF: anyone's code would do): its code is
//! dropped and the browser sent to authorize once more, now with a Nextcloud
//! session, so `state` comes back. The pending login travels in a signed cookie
//! of our own, which survives a pod restart.

use chrono::{DateTime, Duration, Utc};
use rand::Rng;

use crate::session::{sign_value, verify_value};

/// Cookie holding the login in progress. Short-lived; cleared at the callback.
pub const COOKIE_NAME: &str = "oauth_pending";

/// How long a started login may take to come back.
pub fn ttl() -> Duration {
    Duration::seconds(600)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingLogin {
    /// Echoed to NC as `state`; compared back when NC bothers to return it.
    pub nonce: String,
    /// Internal path to land on afterwards; allowlist-validated when used.
    pub return_to: Option<String>,
    pub expires_at: DateTime<Utc>,
    /// Already sent back once for a missing `state`; a second miss is refused.
    pub retried: bool,
}

/// What a callback may do with the pending login it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Callback {
    /// `state` matched: exchange the code.
    Accept(PendingLogin),
    /// `state` was empty: drop the code and authorize again ([`retry`]).
    Retry(PendingLogin),
}

impl PendingLogin {
    /// `<expiry unix>|<nonce>|<retried 0/1>|<return_to>` — `return_to` last, so a
    /// `|` inside a query string can't shift the fields.
    fn encode(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.expires_at.timestamp(),
            self.nonce,
            u8::from(self.retried),
            self.return_to.as_deref().unwrap_or_default()
        )
    }

    fn decode(raw: &str) -> Option<Self> {
        let mut parts = raw.splitn(4, '|');
        let expires_at = DateTime::from_timestamp(parts.next()?.parse().ok()?, 0)?;
        let nonce = parts.next()?.to_string();
        let retried = match parts.next()? {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        let return_to = match parts.next()? {
            "" => None,
            p => Some(p.to_string()),
        };
        Some(Self {
            nonce,
            return_to,
            expires_at,
            retried,
        })
    }
}

fn fresh(
    secret: &str,
    return_to: Option<String>,
    retried: bool,
    now: DateTime<Utc>,
) -> (String, String) {
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    let pending = PendingLogin {
        nonce: hex::encode(bytes),
        return_to,
        expires_at: now + ttl(),
        retried,
    };
    (pending.nonce.clone(), sign_value(secret, &pending.encode()))
}

/// Start a login: a fresh nonce for NC's `state`, plus the signed cookie value that
/// remembers it.
pub fn issue(secret: &str, return_to: Option<String>, now: DateTime<Utc>) -> (String, String) {
    fresh(secret, return_to, false, now)
}

/// The second and last attempt after a missing `state`: a new nonce, the same
/// destination.
pub fn retry(secret: &str, pending: &PendingLogin, now: DateTime<Utc>) -> (String, String) {
    fresh(secret, pending.return_to.clone(), true, now)
}

/// What this callback may do, or `None` if it does not belong to a login this
/// browser started. A present `state` must match; an empty one earns a retry,
/// once.
pub fn accept(
    secret: &str,
    cookie: Option<&str>,
    state: Option<&str>,
    now: DateTime<Utc>,
) -> Option<Callback> {
    let pending = PendingLogin::decode(&verify_value(secret, cookie?)?)?;
    if pending.expires_at < now {
        return None;
    }
    match state {
        Some(s) if !s.is_empty() => (s == pending.nonce).then_some(Callback::Accept(pending)),
        _ => (!pending.retried).then_some(Callback::Retry(pending)),
    }
}
