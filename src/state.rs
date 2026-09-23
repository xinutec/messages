//! Shared application state.
//!
//! The login in progress is NOT held here: it rides in a signed cookie
//! (`pending_login`), so it survives a restart and does not assume one pod.

use std::sync::Arc;

use sqlx::MySqlPool;

use crate::config::Config;
use crate::irc_send::IrcSender;

#[derive(Clone)]
pub struct AppState {
    pub pool: MySqlPool,
    pub cfg: Arc<Config>,
    pub http: reqwest::Client,
    /// `None` when no send key is mounted. Prepared once at boot: it owns the
    /// 0400 copy of the key.
    pub irc: Option<Arc<IrcSender>>,
}

impl AppState {
    pub fn new(
        pool: MySqlPool,
        cfg: Config,
        http: reqwest::Client,
        irc: Option<IrcSender>,
    ) -> Self {
        Self {
            pool,
            cfg: Arc::new(cfg),
            http,
            irc: irc.map(Arc::new),
        }
    }
}
