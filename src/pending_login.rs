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
//! It then returns to the callback with an empty `state`, so a server keyed on
//! `state` cannot complete a login from a cookie-less browser. The pending login
//! therefore travels in a signed cookie of our own, which binds it to the
//! browser as `state` would and survives a pod restart; `state` is still checked
//! when NC returns it.
//!
//! Accepted risk: with an empty `state` the cookie is the only binding, so a
//! login-CSRF needs VPN access and a callback landed in the victim's browser
//! within the 10-minute window.

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
}

impl PendingLogin {
    /// `<expiry unix>|<nonce>|<return_to>` — `return_to` last, so a `|` inside a
    /// query string can't shift the fields.
    fn encode(&self) -> String {
        format!(
            "{}|{}|{}",
            self.expires_at.timestamp(),
            self.nonce,
            self.return_to.as_deref().unwrap_or_default()
        )
    }

    fn decode(raw: &str) -> Option<Self> {
        let mut parts = raw.splitn(3, '|');
        let expires_at = DateTime::from_timestamp(parts.next()?.parse().ok()?, 0)?;
        let nonce = parts.next()?.to_string();
        let return_to = match parts.next()? {
            "" => None,
            p => Some(p.to_string()),
        };
        Some(Self {
            nonce,
            return_to,
            expires_at,
        })
    }
}

/// Start a login: a fresh nonce for NC's `state`, plus the signed cookie value that
/// remembers it.
pub fn issue(secret: &str, return_to: Option<String>, now: DateTime<Utc>) -> (String, String) {
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    let pending = PendingLogin {
        nonce: hex::encode(bytes),
        return_to,
        expires_at: now + ttl(),
    };
    (pending.nonce.clone(), sign_value(secret, &pending.encode()))
}

/// Finish a login: the pending entry if this callback really belongs to it.
///
/// `state` is what NC returned — `None`/empty when it was lost through the Login
/// Flow. Present means it must match; absent means the cookie stands alone.
pub fn accept(
    secret: &str,
    cookie: Option<&str>,
    state: Option<&str>,
    now: DateTime<Utc>,
) -> Option<PendingLogin> {
    let pending = PendingLogin::decode(&verify_value(secret, cookie?)?)?;
    if pending.expires_at < now {
        return None;
    }
    match state {
        Some(s) if !s.is_empty() && s != pending.nonce => None,
        _ => Some(pending),
    }
}
