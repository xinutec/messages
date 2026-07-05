//! JSON API. Every route requires a valid session (the `AuthUser` extractor),
//! which in turn only exists for an allow-listed user (see routes::auth).

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::archive;
use crate::error::AppError;
use crate::session::AuthUser;
use crate::state::AppState;

#[derive(Serialize)]
pub struct Me {
    user_id: String,
    display_name: String,
}

/// GET /api/me → the current session's user (drives the UI login gate).
pub async fn me(AuthUser(user): AuthUser) -> Json<Me> {
    Json(Me {
        user_id: user.user_id,
        display_name: user.display_name,
    })
}

/// GET /api/conversations → all conversations across both origins.
pub async fn conversations(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
) -> Result<Json<Vec<archive::Conversation>>, AppError> {
    Ok(Json(archive::list_conversations(&app.pool).await?))
}

#[derive(Deserialize)]
pub struct MessagesQuery {
    /// Opaque cursor from a previous page's `next_cursor`; absent → newest page.
    cursor: Option<String>,
    limit: Option<i64>,
}

/// GET /api/conversations/{origin}/{id}/messages → one page, oldest→newest.
pub async fn messages(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Path((origin, id)): Path<(String, String)>,
    Query(q): Query<MessagesQuery>,
) -> Result<Json<archive::MessagesPage>, AppError> {
    if !archive::valid_origin(&origin) {
        return Err(AppError::NotFound);
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    // A malformed cursor just falls back to the newest page (treated as absent).
    let cursor = q.cursor.as_deref().and_then(archive::parse_cursor);
    let page = archive::messages_page(&app.pool, &origin, &id, cursor, limit).await?;
    Ok(Json(page))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
    limit: Option<i64>,
}

/// GET /api/attachments/{id} → stream a Signal attachment blob from the PVC.
/// Only serves files whose bytes were downloaded; resolves by basename under
/// the configured attachments dir, so a stored path can't escape the mount.
pub async fn attachment(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let Some((content_type, stored_path)) = archive::attachment_blob(&app.pool, id).await? else {
        return Err(AppError::NotFound);
    };
    let name = std::path::Path::new(&stored_path)
        .file_name()
        .ok_or(AppError::NotFound)?;
    let path = std::path::Path::new(&app.cfg.attachments_dir).join(name);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let ct = content_type.unwrap_or_else(|| "application/octet-stream".to_string());
    Ok(([(header::CONTENT_TYPE, ct)], Body::from(bytes)).into_response())
}

/// GET /api/search?q= → substring search across both origins.
pub async fn search(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Query(sq): Query<SearchQuery>,
) -> Result<Json<Vec<archive::SearchHit>>, AppError> {
    let limit = sq.limit.unwrap_or(50).clamp(1, 200);
    let q = sq.q.trim();
    if q.is_empty() {
        return Ok(Json(Vec::new()));
    }
    Ok(Json(archive::search(&app.pool, q, limit).await?))
}
