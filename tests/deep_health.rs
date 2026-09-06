//! `/healthz/deep` — whether the ARCHIVE answers, not whether this process does.
//!
//! ⚠ **THE FAILING CASE NEEDS NO DATABASE, WHICH IS WHY IT IS THE ONE TESTED
//! HERE.** A pool pointed at nothing is exactly the state this endpoint exists
//! to report, so the unhappy path is reachable in any environment — including
//! the sandbox that builds the package, where a test needing a live server would
//! either be skipped or, worse, pass for the wrong reason.
//!
//! That is not a hypothetical worry. A sibling probe in xinutec-infra was
//! written against a real socket on 2026-09-06 and its POSITIVE case passed in a
//! sandbox with no curl, because "not a failure" was satisfied by "could not
//! ask".
//!
//! The happy path is covered where a database exists: `api_routes.rs` runs
//! against `MESSAGES_TEST_DATABASE_URL`, and the endpoint is exercised live
//! after deploy.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use messages::config::Config;
use messages::state::AppState;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
use tower::ServiceExt;

/// A pool that will never connect: lazy, so building it cannot fail, and
/// pointed at a port nothing listens on.
fn unreachable_archive() -> sqlx::MySqlPool {
    MySqlPoolOptions::new()
        .max_connections(1)
        // Do not spend the probe's whole budget on retries.
        .acquire_timeout(std::time::Duration::from_millis(500))
        .connect_lazy_with(
            MySqlConnectOptions::new()
                .host("127.0.0.1")
                .port(1)
                .username("nobody")
                .database("nothing"),
        )
}

fn cfg() -> Config {
    Config {
        db_options: MySqlConnectOptions::new(),
        session_secret: "test".to_string(),
        bind_addr: String::new(),
        nc_base_url: "https://nc.invalid".to_string(),
        nc_client_id: String::new(),
        nc_client_secret: String::new(),
        nc_redirect_uri: String::new(),
        allowed_users: vec!["pippijn".to_string()],
        static_dir: None,
        attachments_dir: "/nonexistent".to_string(),
        irc_send: None,
    }
}

async fn get(uri: &str) -> StatusCode {
    let app = messages::routes::router(AppState::new(
        unreachable_archive(),
        cfg(),
        reqwest::Client::new(),
        None,
    ));
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    app.oneshot(req).await.unwrap().status()
}

/// ⚠ **503, AND THE NUMBER IS THE CONTRACT.** The fleet's front-door witness
/// counts any 5xx as not serving and everything below 500 as serving, so an
/// unreachable archive has to answer in that vocabulary. A 200 carrying
/// `{"db":false}` would be green in every place that decides anything.
#[tokio::test]
async fn an_unreachable_archive_is_a_server_error() {
    assert_eq!(get("/healthz/deep").await, StatusCode::SERVICE_UNAVAILABLE);
}

/// ⚠ **THE LIVENESS PATH MUST NOT FOLLOW IT DOWN.** `/healthz` is kubelet's
/// liveness target: if it learned to check the database, a database blip would
/// become a crashloop — the pod killed for something a restart cannot fix, and
/// the restarts adding load to whatever was already struggling.
///
/// Same request, same dead pool, and this one must still be 200. Ablating the
/// separation — pointing liveness at the deep check — fails here.
#[tokio::test]
async fn the_liveness_path_stays_up_when_the_archive_is_down() {
    assert_eq!(get("/healthz").await, StatusCode::OK);
}

/// ⚠ **UNAUTHENTICATED ON PURPOSE.** The front door probes it from outside any
/// session, so a 401 would make it unprobeable — and an endpoint the prober
/// cannot read is indistinguishable from an app that is fine.
///
/// This is the one place in the app where "no cookie" must NOT be 401, so it is
/// asserted rather than left to `api_routes.rs`, whose rule is the opposite.
#[tokio::test]
async fn it_answers_without_a_session() {
    for path in ["/healthz", "/healthz/deep"] {
        assert_ne!(
            get(path).await,
            StatusCode::UNAUTHORIZED,
            "{path} must be probeable with no cookie"
        );
    }
}
