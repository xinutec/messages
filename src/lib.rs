//! messages — viewer backend for the multi-origin message archive (Signal,
//! Google Chat and IRC) stored in the `signal` MariaDB. The binary
//! (`src/main.rs`) is a thin wrapper; logic lives here.
//!
//! ⚠ It was read-only by construction until IRC gained a send path, and that is
//! worth stating rather than quietly dropping from this sentence: the app can
//! now act as Pippijn on networks other people are on. [`irc_send`] carries the
//! reasoning about what bounds that.

pub mod archive;
pub mod config;
pub mod db;
pub mod error;
pub mod irc_send;
pub mod nextcloud;
pub mod pending_login;
pub mod routes;
pub mod session;
pub mod state;
