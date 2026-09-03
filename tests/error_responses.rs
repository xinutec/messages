//! What an error tells the caller, and what it must not.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use messages::error::AppError;

async fn body_of(e: AppError) -> (StatusCode, String) {
    let resp = e.into_response();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn each_error_carries_the_status_the_frontend_branches_on() {
    assert_eq!(
        body_of(AppError::Unauthorized).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(body_of(AppError::Forbidden).await.0, StatusCode::FORBIDDEN);
    assert_eq!(body_of(AppError::NotFound).await.0, StatusCode::NOT_FOUND);
}

/// ⚠ **An unexpected error must not describe itself to the caller.** `Other`
/// wraps anything at all — a sqlx failure naming a table and a column, a reqwest
/// error naming an internal host, a message from the irssi box. The detail
/// belongs in the log; the response gets a fixed string. This is the test that
/// notices if someone "improves" the body by including the error.
#[tokio::test]
async fn an_internal_error_says_nothing_about_itself() {
    let (status, body) = body_of(AppError::Other(anyhow::anyhow!(
        "connecting to 10.100.0.2 as user signal_ro failed: password rejected"
    )))
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    for leaked in ["10.100.0.2", "signal_ro", "password"] {
        assert!(!body.contains(leaked), "body leaked {leaked:?}: {body}");
    }
    assert!(body.contains("internal error"));
}
