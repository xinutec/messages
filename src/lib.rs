//! messages — read-only viewer backend for the multi-origin message archive
//! (Signal + Google Chat) stored in the `signal` MariaDB. The binary
//! (`src/main.rs`) is a thin wrapper; logic lives here.

pub mod archive;
pub mod config;
pub mod db;
pub mod error;
pub mod nextcloud;
pub mod routes;
pub mod session;
pub mod state;
