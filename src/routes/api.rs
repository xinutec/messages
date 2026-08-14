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

/// Identity echo for /api/me — drives the UI login gate.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
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
    // An unknown origin is a 404, not a 400: the URL names a conversation that
    // does not exist, and no origin the archive holds is spelled that way.
    let origin = archive::Origin::parse(&origin).ok_or(AppError::NotFound)?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    // A malformed cursor just falls back to the newest page (treated as absent).
    let cursor = q.cursor.as_deref().and_then(archive::parse_cursor);
    let page = archive::messages_page(&app.pool, origin, &id, cursor, limit).await?;
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

/// A message to put on IRC, as Pippijn.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct SendRequest {
    /// ⚠ The body and nothing else. Who it goes to is decided by the
    /// conversation in the URL, looked up in the archive — a request cannot
    /// name a network or a nick, so it cannot address somebody the archive has
    /// never seen.
    pub text: String,
}

/// What happened, in the words of the irssi that did it.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct SendResult {
    /// True only when irssi put the message on the wire.
    pub sent: bool,
    /// Why not, when it did not. This is the far side's refusal — most often
    /// that the recipient is not on the allow-list held on the irssi host.
    pub error: Option<String>,
    /// Whether the message is already in the archive and so will appear without
    /// waiting for the hourly import. A send can succeed with this false.
    pub archived: bool,
}

/// POST /api/conversations/irc/{id}/send → say something, through irssi.
///
/// ⚠ **The only write in the app, and the only thing another person sees.** Two
/// things bound it and neither is in this function: the session must belong to
/// an allow-listed user (the `AuthUser` extractor, as for every route here), and
/// the irssi host decides for itself whether this recipient may be messaged.
/// This handler's own contribution is narrower — it refuses to send anywhere the
/// archive does not already hold a conversation.
pub async fn send(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Path((origin, id)): Path<(String, String)>,
    Json(req): Json<SendRequest>,
) -> Result<Json<SendResult>, AppError> {
    // Only IRC can be sent to. Signal and Google Chat are archives of
    // conversations held elsewhere; there is no live client here to send with,
    // and a 404 says so more honestly than a 400 about an unsupported origin.
    if archive::Origin::parse(&origin) != Some(archive::Origin::Irc) {
        return Err(AppError::NotFound);
    }
    let Some(sender) = app.irc.clone() else {
        // No key mounted. The archive still reads; this capability is simply
        // not configured, which is a server-side fact rather than a bad request.
        return Ok(Json(SendResult {
            sent: false,
            error: Some("sending is not configured".to_string()),
            archived: false,
        }));
    };

    let Some(target) = archive::irc_target(&app.pool, &id).await? else {
        return Err(AppError::NotFound);
    };
    // The pseudo-conversation irssi files server notices into is named after
    // Pippijn's own nick, so it looks exactly like a conversation and is not
    // one. The reader hides it; sending to it would message himself in reply to
    // a server.
    if target.is_status {
        return Err(AppError::NotFound);
    }

    match sender
        .send(&target.network, &target.target, &req.text)
        .await?
    {
        crate::irc_send::Outcome::Refused(why) => Ok(Json(SendResult {
            sent: false,
            error: Some(why),
            archived: false,
        })),
        crate::irc_send::Outcome::Sent(sent) => {
            // ⚠ The message has gone by this point. A failure to record it is
            // therefore logged and not returned: telling the caller the send
            // failed would be false, and would invite them to send it again.
            let archived = match crate::irc_send::record_echo(&app.pool, &id, &sent).await {
                Ok(written) => written,
                Err(e) => {
                    tracing::error!("sent, but could not record the echo: {e:#}");
                    false
                }
            };
            Ok(Json(SendResult {
                sent: true,
                error: None,
                archived,
            }))
        }
    }
}
