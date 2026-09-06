//! Is the archive READABLE — not merely, is this process alive.
//!
//! ⚠ **THE OTHER HEALTH ENDPOINT CANNOT ANSWER THIS, AND MUST NOT LEARN TO.**
//! `/healthz` is `"ok"`, a literal, and it is kubelet's LIVENESS target. A
//! liveness probe that checks a dependency turns a database blip into a
//! crashloop: kubelet kills the pod for something a restart cannot fix, and the
//! restarts add load to whatever was already struggling. So liveness stays dumb
//! and the dependency question lives here.
//!
//! ⚠ **NOT WIRED TO READINESS EITHER, and the name avoids inviting it.** A
//! readiness probe reading this would pull the pod out of its Service whenever
//! the database blinked — turning one shared-database outage into every app
//! backed by it vanishing at once. Called `/healthz/deep` rather than `/readyz`
//! because a name outlives the person who chose it, and `readyz` is an
//! instruction to wire it somewhere it must not go.
//!
//! ⚠ **UNAUTHENTICATED, deliberately.** The front door probes it from outside
//! any session (`plan-run frontdoor`), so a gate here would make it unprobeable
//! — and what it discloses is one bit, whether this pod can reach its database,
//! on a VPN-only name whose archive is already behind SSO.
//!
//! Why it exists at all: messages.xinutec.org answered 502 for 26 hours on
//! 2026-09-04/05 because its pod was dead, and nothing noticed. The fleet now
//! asks every fronted name for an HTTP status — which sees a dead process, and
//! would see NOTHING here if the process were fine and the archive unreachable,
//! because `/` is the Angular bundle served out of this same process.

use axum::extract::State;
use axum::http::StatusCode;
use std::time::Duration;

use crate::state::AppState;

/// How long the archive may take to answer before it counts as unreachable.
///
/// Short on purpose. This is a liveness question about a dependency, not a
/// query, and a probe that waits 30 s reports a hung database as a timeout at
/// the far end — which reads as "the prober could not ask" rather than "the app
/// cannot work". `plan-run`'s own curl gives up at 8 s, so anything near that
/// would be answered by the wrong side.
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(3);

/// GET /healthz/deep → 200 when the archive answers, 503 when it does not.
///
/// ⚠ **503 IS THE POINT.** The fleet's front-door witness counts any 5xx as not
/// serving and anything below 500 as serving, so this must answer in that
/// vocabulary rather than with a JSON body nobody parses. A 200 with
/// `{"db":false}` inside would be green everywhere that matters.
///
/// `SELECT 1` rather than a query against the archive's own tables: it proves
/// the pool can reach the server and authenticate, which is the failure this
/// exists for (the database down, the credentials rotated, the network
/// partitioned). Reading a real table would also fail when a table is merely
/// empty, and an empty archive is not a broken one.
pub async fn deep(State(app): State<AppState>) -> (StatusCode, &'static str) {
    let probe = sqlx::query("SELECT 1").fetch_one(&app.pool);
    match tokio::time::timeout(ARCHIVE_TIMEOUT, probe).await {
        Ok(Ok(_)) => (StatusCode::OK, "archive reachable\n"),
        Ok(Err(e)) => {
            tracing::warn!("deep health: the archive refused a trivial query: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "archive unreachable\n")
        }
        Err(_) => {
            tracing::warn!(
                "deep health: the archive did not answer SELECT 1 within {}s",
                ARCHIVE_TIMEOUT.as_secs()
            );
            (StatusCode::SERVICE_UNAVAILABLE, "archive not answering\n")
        }
    }
}
