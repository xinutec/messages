//! DB-backed sessions. Nextcloud is touched only at login; every later request
//! authenticates against this opaque session. Cookie: `<id>.<hex hmac_sha256(id)>`,
//! verified constant-time. Copied from the `life` app.

use anyhow::Result;
use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum_extra::extract::cookie::CookieJar;
use chrono::{Duration, NaiveDateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use rand::Rng;
use sha2::Sha256;
use sqlx::MySqlPool;

use crate::error::AppError;
use crate::state::AppState;

type HmacSha256 = Hmac<Sha256>;

const SESSION_TTL_DAYS: i64 = 7;
pub const COOKIE_NAME: &str = "session";

#[derive(Clone, Debug)]
pub struct UserSession {
    pub user_id: String,
    pub display_name: String,
}

pub fn sign_value(secret: &str, value: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(value.as_bytes());
    format!("{value}.{}", hex::encode(mac.finalize().into_bytes()))
}

/// Verify a signed cookie, returning the inner id (None if bad). Constant-time.
pub fn verify_value(secret: &str, signed: &str) -> Option<String> {
    let idx = signed.rfind('.')?;
    let (value, dotted_sig) = signed.split_at(idx);
    let sig = hex::decode(&dotted_sig[1..]).ok()?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(value.as_bytes());
    mac.verify_slice(&sig).ok()?;
    Some(value.to_string())
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    user_id: String,
    display_name: String,
    expires_at: NaiveDateTime,
}

/// Create a session row and return the signed cookie value.
pub async fn create_session(pool: &MySqlPool, secret: &str, user: &UserSession) -> Result<String> {
    let mut id_bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut id_bytes);
    let id = hex::encode(id_bytes);
    let expires = (Utc::now() + Duration::days(SESSION_TTL_DAYS)).naive_utc();
    sqlx::query("INSERT INTO sessions (id, user_id, display_name, expires_at) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(&user.user_id)
        .bind(&user.display_name)
        .bind(expires)
        .execute(pool)
        .await?;
    Ok(sign_value(secret, &id))
}

/// Resolve a signed cookie to a session. Lazily deletes the row if expired.
pub async fn get_session(
    pool: &MySqlPool,
    secret: &str,
    signed: &str,
) -> Result<Option<UserSession>> {
    let Some(id) = verify_value(secret, signed) else {
        return Ok(None);
    };
    let row: Option<SessionRow> =
        sqlx::query_as("SELECT user_id, display_name, expires_at FROM sessions WHERE id = ?")
            .bind(&id)
            .fetch_optional(pool)
            .await?;
    let Some(row) = row else { return Ok(None) };
    if row.expires_at < Utc::now().naive_utc() {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&id)
            .execute(pool)
            .await?;
        return Ok(None);
    }
    Ok(Some(UserSession {
        user_id: row.user_id,
        display_name: row.display_name,
    }))
}

/// Delete the session backing a signed cookie (logout).
pub async fn destroy_session(pool: &MySqlPool, secret: &str, signed: &str) -> Result<()> {
    if let Some(id) = verify_value(secret, signed) {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Extractor: rejects with 401 unless a valid session cookie is present.
pub struct AuthUser(pub UserSession);

impl<S> FromRequestParts<S> for AuthUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app = AppState::from_ref(state);
        let jar = CookieJar::from_headers(&parts.headers);
        let cookie = jar.get(COOKIE_NAME).ok_or(AppError::Unauthorized)?;
        let user = get_session(&app.pool, &app.cfg.session_secret, cookie.value())
            .await?
            .ok_or(AppError::Unauthorized)?;
        Ok(AuthUser(user))
    }
}
