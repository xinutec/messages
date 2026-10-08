//! Auth routes: Nextcloud identity login, restricted to an explicit allow-list.
//!
//! A Nextcloud user not on the allow-list is rejected with 403 and gets no
//! session.

use anyhow::anyhow;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::Utc;
use serde::Deserialize;

use crate::error::AppError;
use crate::nextcloud::identity;
use crate::pending_login;
use crate::session::{COOKIE_NAME, SESSION_TTL_DAYS, UserSession, create_session, destroy_session};
use crate::state::AppState;

fn session_cookie(value: String) -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, value))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::days(SESSION_TTL_DAYS))
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
///
/// A browser reads `/\host` as `//host`, another site, so a backslash in the
/// second place is refused like a slash.
pub fn validate_return_to(return_to: Option<&str>) -> String {
    match return_to {
        Some(p) if p.starts_with('/') && !p[1..].starts_with(['/', '\\']) => p.to_string(),
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

/// The OAuth callback: where Nextcloud sends the browser. A failure is drawn as
/// a page, because `AppError`'s JSON in its place reads as the app being broken.
pub async fn callback(
    State(app): State<AppState>,
    jar: CookieJar,
    Query(q): Query<CallbackQuery>,
) -> Response {
    match finish_sign_in(app, jar, q).await {
        Ok(done) => done.into_response(),
        Err(e) => sign_in_problem(e.into_response().status()),
    }
}

/// A sign-in that could not be finished, said in words and with a way back in.
fn sign_in_problem(status: StatusCode) -> Response {
    let said = match status {
        StatusCode::UNAUTHORIZED => "This sign-in did not start here, or it took too long.",
        StatusCode::FORBIDDEN => "This account may not use this app.",
        _ => "The sign-in could not be finished.",
    };
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>Sign-in did not finish</title><style>\
         body{{font:16px/1.5 system-ui,-apple-system,sans-serif;margin:0;\
         min-height:100vh;display:grid;place-items:center;padding:1.5rem;color:#1a1a1a}}\
         main{{max-width:26rem}}h1{{font-size:1.2rem;margin:0 0 .5rem}}\
         p{{margin:0 0 1.5rem;color:#555}}\
         a{{display:inline-block;padding:.65rem 1.1rem;border-radius:.5rem;\
         background:#1b6ac9;color:#fff;text-decoration:none}}\
         </style></head><body><main><h1>Sign-in did not finish</h1>\
         <p>{said}</p><a href=\"/login\">Try again</a></main></body></html>"
    );
    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

/// The callback's work: exchange code, read identity, ENFORCE the allow-list,
/// then create our session.
async fn finish_sign_in(
    app: AppState,
    jar: CookieJar,
    q: CallbackQuery,
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

/// POST /logout → destroy the session + clear the cookie; 204, the page reloads itself.
pub async fn logout(
    State(app): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, StatusCode), AppError> {
    if let Some(c) = jar.get(COOKIE_NAME) {
        destroy_session(&app.pool, &app.cfg.session_secret, c.value()).await?;
    }
    Ok((
        jar.remove(Cookie::from(COOKIE_NAME)),
        StatusCode::NO_CONTENT,
    ))
}
