//! Staging the IRC send key into its scratch directory.
//!
//! No database, no network; sending itself needs irssi.

use messages::config::IrcSend;
use messages::irc_send::IrcSender;
use std::path::{Path, PathBuf};

/// A secret directory and a scratch directory under cargo's test target dir,
/// since TMPDIR can be unreadable under `nix develop`.
fn dirs(case: &str) -> (PathBuf, PathBuf) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(case);
    let (keys, work) = (root.join("secret"), root.join("run"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&keys).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(keys.join("id_ed25519"), b"not a real key, and never used\n").unwrap();
    std::fs::write(
        keys.join("known_hosts"),
        b"[10.100.0.1]:2230 ssh-ed25519 AAAA\n",
    )
    .unwrap();
    (keys, work)
}

fn cfg(keys: &Path, work: &Path) -> IrcSend {
    IrcSend {
        host: "10.100.0.1".to_string(),
        port: 2230,
        key_dir: keys.display().to_string(),
        work_dir: work.display().to_string(),
    }
}

/// The second start: the scratch dir survives a container restart, and the
/// key there is 0400.
#[tokio::test]
async fn staging_the_key_survives_a_container_restart() {
    let (keys, work) = dirs("restart");
    let cfg = cfg(&keys, &work);

    assert!(
        IrcSender::prepare(&cfg).await.unwrap().is_some(),
        "first start"
    );
    let staged = work.join("id_ed25519");
    assert!(staged.exists());

    assert!(
        IrcSender::prepare(&cfg).await.unwrap().is_some(),
        "a restart must not be fatal — this is the 26-hour outage"
    );

    // Staged anew, since the secret may have rotated.
    assert_eq!(
        std::fs::read(&staged).unwrap(),
        std::fs::read(keys.join("id_ed25519")).unwrap()
    );
}

/// ssh refuses a private key with any group or other bit.
#[cfg(unix)]
#[tokio::test]
async fn the_staged_key_is_readable_only_by_its_owner() {
    use std::os::unix::fs::PermissionsExt;
    let (keys, work) = dirs("mode");

    IrcSender::prepare(&cfg(&keys, &work)).await.unwrap();

    let mode = std::fs::metadata(work.join("id_ed25519"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o400, "0400, or ssh refuses the key");
}

/// A missing key disables sending and leaves the archive served. (A key that
/// cannot be staged, handled in `main.rs`, is not reachable from a test.)
#[tokio::test]
async fn no_key_means_no_sending_rather_than_no_service() {
    let (keys, work) = dirs("absent");
    std::fs::remove_file(keys.join("id_ed25519")).unwrap();

    assert!(
        IrcSender::prepare(&cfg(&keys, &work))
            .await
            .unwrap()
            .is_none()
    );
}
