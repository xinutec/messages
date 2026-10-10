//! A conversation's picture, from the files kept beside the media already
//! stored, each in an `avatars` directory: the Signal ingester's and the Telegram
//! feed's, and Google Chat's from gchat-archive's `fetch_avatars.py`. Found by
//! name; no file is no picture, and the list draws initials.

use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use crate::archive::{Conversation, Origin};
use crate::config::Config;

/// Where a conversation's picture is filed under the two media dirs, or `None`
/// where the origin keeps none or the name would leave the directory. Whoever
/// writes them uses the same names.
fn filed(attachments: &str, telegram: &str, origin: Origin, id: &str) -> Option<PathBuf> {
    let (dir, name) = match origin {
        // The thread id, with a group id's `/` as `_`.
        Origin::Signal => (attachments, id.replace('/', "_")),
        Origin::Gchat => (attachments, format!("gchat-{id}.jpg")),
        Origin::Telegram => (telegram, format!("{id}.jpg")),
        Origin::Irc => return None,
    };
    if name.contains('/') || name.starts_with('.') {
        return None;
    }
    Some(PathBuf::from(dir).join("avatars").join(name))
}

/// The picture's file and when it last changed, in epoch seconds: the version
/// its URL carries, so a new picture is a new URL.
pub fn find(cfg: &Config, origin: Origin, id: &str) -> Option<(PathBuf, i64)> {
    let path = filed(&cfg.attachments_dir, &cfg.telegram_media_dir, origin, id)?;
    let changed = std::fs::metadata(&path).ok()?.modified().ok()?;
    let secs = changed.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some((path, i64::try_from(secs).ok()?))
}

/// Each conversation's picture version, for the list.
pub fn attach(cfg: &Config, conversations: &mut [Conversation]) {
    for c in conversations {
        c.avatar = find(cfg, c.origin, &c.id).map(|(_, version)| version);
    }
}

/// What a picture is, from its first bytes: the names carry no extension to
/// trust.
pub fn image_type(head: &[u8]) -> &'static str {
    if head.starts_with(b"\x89PNG") {
        "image/png"
    } else if head.starts_with(b"GIF8") {
        "image/gif"
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/jpeg"
    }
}
