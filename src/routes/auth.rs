//! Auth routes: Nextcloud identity login, restricted to an explicit allow-list.
//!
//! The login flow is copied from `life`; the one addition is the allow-list
//! check in the callback — the archive holds private messages and the host is
//! on a shared VPN, so a successfully-authenticated but non-allowed Nextcloud
//! user is rejected with 403 and no session is minted.

use anyhow::anyhow;
use axum::extract::{Query, State};
use axum::response::Redirect;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::Utc;
use serde::Deserialize;

use crate::error::AppError;
use crate::nextcloud::identity;
use crate::pending_login;
use crate::session::{COOKIE_NAME, UserSession, create_session, destroy_session};
use crate::state::AppState;

fn session_cookie(value: String) -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, value))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::days(7))
        .build()
}

/// The login-in-progress cookie. `Lax`, because the callback arrives as a
/// top-level navigation from Nextcloud and a `Strict` cookie would not be sent.
fn pending_cookie(value: String) -> Cookie<'static> {
    Cookie::build((pending_login::COOKIE_NAME, value))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(pending_login::ttl().num_seconds()))
        .build()
}

/// Only allow same-site internal paths as a post-login redirect target.
pub fn validate_return_to(return_to: Option<&str>) -> String {
    match return_to {
        Some(p) if p.starts_with('/') && !p.starts_with("//") => p.to_string(),
        _ => "/".to_string(),
    }
}

#[derive(Deserialize)]
pub struct LoginQuery {
    return_to: Option<String>,
}

/// GET /login → redirect to NC's OAuth2 authorize endpoint, remembering the login
/// in a signed cookie (see [`pending_login`] for why the `state` echo isn't enough).
pub async fn login(
    State(app): State<AppState>,
    jar: CookieJar,
    Query(q): Query<LoginQuery>,
) -> (CookieJar, Redirect) {
    let (nonce, cookie) = pending_login::issue(&app.cfg.session_secret, q.return_to, Utc::now());
    (
        jar.add(pending_cookie(cookie)),
        Redirect::to(&identity::authorize_url(&app.cfg, &nonce)),
    )
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

/// GET /auth/callback → exchange code, read identity, ENFORCE the allow-list,
/// then create our session.
pub async fn callback(
    State(app): State<AppState>,
    jar: CookieJar,
    Query(q): Query<CallbackQuery>,
) -> Result<(CookieJar, Redirect), AppError> {
    let pending = pending_login::accept(
        &app.cfg.session_secret,
        jar.get(pending_login::COOKIE_NAME).map(Cookie::value),
        q.state.as_deref(),
        Utc::now(),
    )
    .ok_or(AppError::Unauthorized)?;
    let code = q
        .code
        .ok_or_else(|| anyhow!("missing authorization code"))?;

    let token = identity::exchange_code(&app.http, &app.cfg, &code).await?;
    let nc_user = identity::fetch_user(&app.http, &app.cfg, &token).await?;

    if !app.cfg.is_allowed(&nc_user.id) {
        tracing::warn!(
            "denied login for non-allowed Nextcloud user {:?}",
            nc_user.id
        );
        return Err(AppError::Forbidden);
    }

    let user = UserSession {
        user_id: nc_user.id,
        display_name: nc_user.display_name,
    };
    let signed = create_session(&app.pool, &app.cfg.session_secret, &user).await?;
    let dest = validate_return_to(pending.return_to.as_deref());
    // The login is over: drop its cookie so a stale one can't be replayed.
    let jar = jar.remove(Cookie::from(pending_login::COOKIE_NAME));
    Ok((jar.add(session_cookie(signed)), Redirect::to(&dest)))
}

/// POST /logout → destroy the session + clear the cookie.
pub async fn logout(
    State(app): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, Redirect), AppError> {
    if let Some(c) = jar.get(COOKIE_NAME) {
        destroy_session(&app.pool, &app.cfg.session_secret, c.value()).await?;
    }
    Ok((jar.remove(Cookie::from(COOKIE_NAME)), Redirect::to("/")))
}
