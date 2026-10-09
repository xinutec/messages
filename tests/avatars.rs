//! Where a conversation's picture is found: the names the feeds file them
//! under, and none that leaves the directory.

#[path = "support/config.rs"]
mod support;

use messages::archive::Origin;
use messages::avatars::{find, image_type};
use messages::config::Config;

/// Two media dirs with an `avatars` directory each, holding `files`.
fn dirs(test: &str, files: &[(&str, &str)]) -> (Config, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("messages-{test}-{}", std::process::id()));
    for (dir, name) in files {
        let at = root.join(dir).join("avatars");
        std::fs::create_dir_all(&at).unwrap();
        std::fs::write(at.join(name), b"picture").unwrap();
    }
    let cfg = Config {
        attachments_dir: root.join("a").to_string_lossy().into_owned(),
        telegram_media_dir: root.join("t").to_string_lossy().into_owned(),
        ..support::config()
    };
    (cfg, root)
}

#[test]
fn signal_is_filed_by_thread_id_with_slashes_made_underscores() {
    let (cfg, root) = dirs("signal", &[("a", "group:jEsJ_yV7+Q="), ("a", "dm:u-1")]);
    assert!(find(&cfg, Origin::Signal, "group:jEsJ/yV7+Q=").is_some());
    assert!(find(&cfg, Origin::Signal, "dm:u-1").is_some());
    assert!(find(&cfg, Origin::Signal, "dm:u-2").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn telegram_is_filed_by_conversation_id() {
    let (cfg, root) = dirs("telegram", &[("t", "-1001.jpg")]);
    assert!(find(&cfg, Origin::Telegram, "-1001").is_some());
    // Each origin looks in its own dir.
    assert!(find(&cfg, Origin::Signal, "-1001.jpg").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn no_name_leaves_the_directory() {
    // A file beside `avatars`, which a `..` would reach.
    let (cfg, root) = dirs("escape", &[("t", "x.jpg")]);
    std::fs::write(root.join("t").join("secret.jpg"), b"not a picture").unwrap();
    assert!(find(&cfg, Origin::Telegram, "../secret").is_none());
    assert!(find(&cfg, Origin::Signal, "..").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn origins_without_pictures_have_none() {
    let (cfg, root) = dirs("none", &[("a", "7"), ("a", "g")]);
    assert!(find(&cfg, Origin::Irc, "7").is_none());
    assert!(find(&cfg, Origin::Gchat, "g").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_type_comes_from_the_bytes() {
    assert_eq!(image_type(b"\x89PNG\r\n\x1a\n"), "image/png");
    assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), "image/webp");
    assert_eq!(image_type(b"\xff\xd8\xff\xe0"), "image/jpeg");
}
