//! The queue recall's transcriber polls, at the paths its `runner` already
//! uses; see `transcribe.rs`. Every route takes the transcriber's bearer token
//! and nothing else: it is no reader, and has no session.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::archive;
use crate::error::AppError;
use crate::state::AppState;
use crate::transcribe;

/// Whether the request carries the transcriber's token. Compared in constant
/// time, so the comparison's duration says nothing about the token. With no
/// token configured, the routes answer as if they did not exist.
fn authorised(app: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let expected = app
        .cfg
        .transcriber_token
        .as_deref()
        .ok_or(AppError::NotFound)?;
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let same = given.len() == expected.len()
        && given
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |d, (a, b)| d | (a ^ b))
            == 0;
    if same {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}

/// PUT /work/v1/lease → `{job}`, `null` when there is nothing to do. Only
/// `transcribe-segment` is done here, whatever kinds the runner names.
pub async fn lease(State(app): State<AppState>, headers: HeaderMap) -> Result<Response, AppError> {
    authorised(&app, &headers)?;
    let job = transcribe::lease(&app.pool).await?;
    Ok(Json(json!({ "job": job })).into_response())
}

/// GET /ingest/v1/blob/{source}/{filename} → an attachment's audio. Audio only:
/// the token reads nothing else of the archive.
pub async fn blob(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path((source, filename)): Path<(String, String)>,
) -> Result<Response, AppError> {
    authorised(&app, &headers)?;
    let id: i64 = filename.parse().map_err(|_| AppError::NotFound)?;
    let held = match archive::Origin::parse(&source) {
        Some(archive::Origin::Signal) => archive::signal::attachment_blob(&app.pool, id).await?,
        Some(archive::Origin::Gchat) => archive::gchat::attachment_blob(&app.pool, id).await?,
        Some(archive::Origin::Telegram) => archive::telegram::media_blob(&app.pool, id).await?,
        _ => None,
    };
    let Some((Some(ct), stored)) =
        held.filter(|(ct, _)| ct.as_deref().is_some_and(|c| c.starts_with("audio/")))
    else {
        return Err(AppError::NotFound);
    };
    let dir = if source == "telegram" {
        &app.cfg.telegram_media_dir
    } else {
        &app.cfg.attachments_dir
    };
    super::api::serve_held(
        &format!("{source} audio {id}"),
        dir,
        &stored,
        Some(ct),
        headers,
    )
    .await
}

/// PUT /work/v1/jobs/{id}/done → the runner's outcome, stored.
pub async fn done(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    authorised(&app, &headers)?;
    let finished: transcribe::Finished = serde_json::from_slice(&body)
        .map_err(|e| AppError::Other(anyhow::anyhow!("unreadable outcome: {e}")))?;
    if transcribe::finish(&app.pool, id, &finished).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

/// GET /sync/vocabulary/prompt → no vocabulary: messages biases the model with
/// nothing. recall's runner does not ask (it serves this queue unbiased); the
/// answer is here for any runner that does.
pub async fn prompt(State(app): State<AppState>, headers: HeaderMap) -> Result<Response, AppError> {
    authorised(&app, &headers)?;
    Ok(Json(json!({ "prompt": null })).into_response())
}
