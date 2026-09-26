//! signal-archiver library: pure parsing (`parse` for Signal frames,
//! `telegram::map` for Telegram; irssi autologs are the `irclog` crate) and the
//! MariaDB store (`db`). The binaries connect these to their feeds.

pub mod attach;
pub mod db;
pub mod parse;
pub mod telegram;
