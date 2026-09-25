//! JSON API. Every route requires a valid session (the `AuthUser` extractor),
//! which in turn only exists for an allow-listed user (see routes::auth).

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::archive;
use crate::error::AppError;
use crate::link_fetch;
use crate::session::AuthUser;
use crate::state::AppState;

// ts-rs copies `///` on exported types into `generated/`, where it is the
// frontend's only documentation; Rust-only notes go in `//`.
/// The signed-in user, as the UI's login gate reads it.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct Me {
    user_id: String,
    display_name: String,
}

/// GET /api/me → the current session's user. Drives the UI login gate.
pub async fn me(AuthUser(user): AuthUser) -> Json<Me> {
    Json(Me {
        user_id: user.user_id,
        display_name: user.display_name,
    })
}

/// GET /api/conversations → all conversations across all origins.
pub async fn conversations(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
) -> Result<Json<Vec<archive::Conversation>>, AppError> {
    Ok(Json(archive::list_conversations(&app.pool).await?))
}

#[derive(Deserialize)]
pub struct MessagesQuery {
    /// Opaque cursor from a previous page: its `next_cursor` to continue
    /// backwards, its `prev_cursor` to continue forwards. Absent: the newest page.
    cursor: Option<String>,
    limit: Option<i64>,
    /// `older` (the default), `newer`, or `at`. `newer` is strictly after the
    /// cursor, for scrolling; `at` includes it, for landing. Anything else is
    /// `older`, since absent and misspelled look the same in a query string.
    dir: Option<String>,
    /// Land on the first message of this day: epoch milliseconds for the
    /// reader's local midnight, converted to the origin's unit. Overrides
    /// `cursor`, and leaves `dir` alone so the caller composes a landing from
    /// `older` + `at`.
    on: Option<i64>,
}

/// GET /api/conversations/{origin}/{id}/messages → one page, oldest→newest.
pub async fn messages(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Path((origin, id)): Path<(String, String)>,
    Query(q): Query<MessagesQuery>,
) -> Result<Json<archive::MessagesPage>, AppError> {
    // An unknown origin names a conversation that does not exist: 404.
    let origin = archive::Origin::parse(&origin).ok_or(AppError::NotFound)?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    // A malformed cursor counts as absent.
    let cursor = q.cursor.as_deref().and_then(archive::parse_cursor);
    let dir = match q.dir.as_deref() {
        Some("newer") => archive::PageDir::Newer,
        Some("at") => archive::PageDir::AtAndNewer,
        _ => archive::PageDir::Older,
    };
    let cursor = match q.on {
        Some(ms) => archive::parse_cursor(&archive::cursor_for_day(origin, ms)),
        None => cursor,
    };
    let page = archive::messages_page(&app.pool, origin, &id, cursor, limit, dir).await?;
    Ok(Json(page))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
    limit: Option<i64>,
    /// Narrow to one conversation; both or neither.
    origin: Option<String>,
    id: Option<String>,
}

/// What became of a link: `offered`, `wanted`, `ok`, `not_image` or `failed`,
/// and the type once there is a picture.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct LinkImageState {
    pub state: String,
    pub content_type: Option<String>,
}

/// Serve bytes the archive says it holds.
///
/// By the basename of the stored name under `dir`, so a stored path cannot
/// escape the mount. The row claims the bytes exist, so a read failure means the
/// mount and the archive disagree: logged, and a 404 for the client.
async fn serve_held(
    what: &str,
    dir: &str,
    stored: &str,
    content_type: Option<String>,
) -> Result<Response, AppError> {
    let Some(name) = std::path::Path::new(stored).file_name() else {
        tracing::warn!("{what}: the stored name names no file: {stored:?}");
        return Err(AppError::NotFound);
    };
    let path = std::path::Path::new(dir).join(name);
    let bytes = tokio::fs::read(&path).await.map_err(|e| {
        tracing::warn!(
            "{what}: recorded as held, but reading {} failed: {e}",
            path.display()
        );
        AppError::NotFound
    })?;
    let ct = content_type.unwrap_or_else(|| "application/octet-stream".to_string());
    Ok(([(header::CONTENT_TYPE, ct)], Body::from(bytes)).into_response())
}

/// GET /api/gchat-attachments/{id} → a Google Chat picture from the PVC.
///
/// Each origin's attachment ids are independent, hence a route per origin.
pub async fn gchat_attachment(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let Some((content_type, stored)) = archive::gchat_attachment_blob(&app.pool, id).await? else {
        return Err(AppError::NotFound);
    };
    let what = format!("gchat attachment {id}");
    serve_held(&what, &app.cfg.attachments_dir, &stored, content_type).await
}

/// GET /api/attachments/{id} → a Signal attachment from the PVC.
pub async fn attachment(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let Some((content_type, stored)) = archive::attachment_blob(&app.pool, id).await? else {
        return Err(AppError::NotFound);
    };
    let what = format!("attachment {id}");
    serve_held(&what, &app.cfg.attachments_dir, &stored, content_type).await
}

/// GET /api/telegram-media/{id} → bytes the archive holds for a Telegram message.
///
/// `{id}` is the message's API id. The join keeps the lookup inside that
/// message's conversation.
pub async fn telegram_media(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let Some((content_type, stored)) = archive::telegram_media_blob(&app.pool, id).await? else {
        return Err(AppError::NotFound);
    };
    let what = format!("telegram media {id}");
    serve_held(&what, &app.cfg.telegram_media_dir, &stored, content_type).await
}

/// GET /api/telegram-media/{id}/state → has it arrived yet?
///
/// Polled by a reader waiting on a requested file; the fetch happens later, in
/// another process.
pub async fn telegram_media_state(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<archive::MediaState>, AppError> {
    match archive::telegram_media_state(&app.pool, id).await? {
        Some(state) => Ok(Json(state)),
        None => Err(AppError::NotFound),
    }
}

/// POST /api/telegram-media/{id}/request → a reader asked for these bytes.
///
/// Queues a row for the Telegram feed, the only process holding a session, and
/// returns. The feed accepts no connections, so it polls. 204 whether or not
/// anything was queued.
pub async fn request_telegram_media(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    if archive::request_telegram_media(&app.pool, id).await? {
        tracing::info!("telegram media {id} requested");
    }
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/link-images/{id} → the picture we hold for a link in a message.
///
/// The reader never contacts the linked server: the bytes come from us or not
/// at all. `id` is the URL's SHA-256, which is also the file name.
pub async fn link_image(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let Some((content_type, stored)) = archive::link_image_blob(&app.pool, &id).await? else {
        return Err(AppError::NotFound);
    };
    let what = format!("link image {id}");
    serve_held(&what, &app.cfg.link_images_dir, &stored, Some(content_type)).await
}

/// POST /api/link-images/{id}/request → a reader tapped "show this picture".
///
/// The browser names a hash, never an address, so this cannot be used as a
/// fetch-anything proxy. 404 when the link was never offered or is decided.
pub async fn request_link_image(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<LinkImageState>, AppError> {
    let Some(url) = archive::offered_url(&app.pool, &id).await? else {
        return Err(AppError::NotFound);
    };
    let url = url::Url::parse(&url).map_err(|_| AppError::NotFound)?;

    // Synchronous: the reader is waiting. The fetch runs in the link-fetch pod.
    let outcome = link_fetch::resolve_one(
        &app.pool,
        &app.http,
        &app.cfg.link_fetcher_url,
        std::path::Path::new(&app.cfg.link_images_dir),
        &url,
    )
    .await?;
    let (state, content_type) = match outcome {
        link_fetch::Outcome::Ok => {
            let held = archive::link_image_state(&app.pool, &id).await?;
            ("ok".to_string(), held.and_then(|(_, ct)| ct))
        }
        link_fetch::Outcome::NotImage => ("not_image".to_string(), None),
        link_fetch::Outcome::Failed => ("failed".to_string(), None),
    };
    Ok(Json(LinkImageState {
        state,
        content_type,
    }))
}

/// GET /api/search?q= → substring search across all origins, or inside one
/// conversation with `&origin=&id=`. A partial or unknown scope is a 404 rather
/// than a silent global search.
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
    let scope = match (sq.origin.as_deref(), sq.id.as_deref()) {
        (None, None) => archive::SearchScope::Everywhere,
        (Some(o), Some(id)) if !id.is_empty() => {
            let Some(origin) = archive::Origin::parse(o) else {
                return Err(AppError::NotFound);
            };
            archive::SearchScope::Conversation { origin, id }
        }
        _ => return Err(AppError::NotFound),
    };
    Ok(Json(archive::search(&app.pool, q, limit, scope).await?))
}

/// A message to put on IRC, as Pippijn.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct SendRequest {
    /// The body only. The recipient comes from the conversation in the URL, so
    /// a request cannot address anyone the archive has not seen.
    pub text: String,
}

/// What happened, in the words of the irssi that did it.
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct SendResult {
    /// True only when irssi put the message on the wire.
    pub sent: bool,
    /// Why not: irssi's refusal, usually that it has no tab for the target.
    pub error: Option<String>,
    /// Whether the message is already archived, visible without waiting for the
    /// import. A send can succeed with this false.
    pub archived: bool,
}

/// POST /api/conversations/irc/{id}/send → say something, through irssi.
///
/// The app's one outward write. Bounded by the allow-listed session and by the
/// irssi host's own rules; this handler refuses any target the archive does not
/// hold.
pub async fn send(
    State(app): State<AppState>,
    AuthUser(_user): AuthUser,
    Path((origin, id)): Path<(String, String)>,
    Json(req): Json<SendRequest>,
) -> Result<Json<SendResult>, AppError> {
    // Only IRC. Signal is read-only by decision: an IRC echo can be confirmed
    // against irssi's log, while a Signal echo would be the only evidence the
    // message existed (#900).
    if archive::Origin::parse(&origin) != Some(archive::Origin::Irc) {
        return Err(AppError::NotFound);
    }
    let Some(sender) = app.irc.clone() else {
        // No key mounted: sending is not configured.
        return Ok(Json(SendResult {
            sent: false,
            error: Some("sending is not configured".to_string()),
            archived: false,
        }));
    };

    let Some(target) = archive::irc_target(&app.pool, &id).await? else {
        return Err(AppError::NotFound);
    };
    // irssi's server-notice window is named after Pippijn's nick.
    if target.is_status {
        return Err(AppError::NotFound);
    }

    // Parsed here, so every caller gets the same slash handling.
    let (text, is_action) = crate::irc_send::parse_slash(&req.text);
    match sender
        .send(&target.network, &target.target, text, is_action)
        .await?
    {
        // Both outcomes are 200, so both are logged: target, never the message.
        crate::irc_send::Outcome::Refused(why) => {
            tracing::info!(
                "irc send refused by irssi: {}/{} — {why}",
                target.network,
                target.target
            );
            Ok(Json(SendResult {
                sent: false,
                error: Some(why),
                archived: false,
            }))
        }
        crate::irc_send::Outcome::Sent(sent) => {
            // The message has gone, so a failed echo is logged, not returned.
            let archived = match crate::irc_send::record_echo(&app.pool, &id, &sent).await {
                Ok(written) => written,
                Err(e) => {
                    tracing::error!("sent, but could not record the echo: {e:#}");
                    false
                }
            };
            tracing::info!(
                "irc send ok: {}/{} as {} — {}",
                sent.tag,
                target.target,
                sent.nick,
                if archived {
                    "echo archived, visible now"
                } else {
                    "echo not archived, waits for the hourly import"
                }
            );
            Ok(Json(SendResult {
                sent: true,
                error: None,
                archived,
            }))
        }
    }
}
