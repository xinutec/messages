//! messages — the viewer's backend, over the Signal, Google Chat, IRC and
//! Telegram archive in the `signal` MariaDB. The binary (`src/main.rs`) is a thin
//! wrapper; logic lives here.
//!
//! ⚠ It can send on IRC, as Pippijn, on networks other people are on;
//! [`irc_send`] says what bounds that.

pub mod archive;
pub mod config;
pub mod db;
pub mod error;
pub mod irc_send;
pub mod link_fetch;
pub mod link_image;
pub mod nextcloud;
pub mod pending_login;
pub mod routes;
pub mod session;
pub mod state;
