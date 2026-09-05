//! Staging the IRC send key into its scratch directory.
//!
//! No database and no network: `prepare` copies a key out of the mounted secret
//! and tightens it, which is all of what these exercise. The send itself needs
//! irssi at the other end and is not reachable from a test.

use messages::config::IrcSend;
use messages::irc_send::IrcSender;
use std::path::{Path, PathBuf};

/// A mounted-secret directory and a scratch directory, under the target dir
/// cargo hands to integration tests. Deliberately not `/tmp`: under `nix
/// develop` TMPDIR has been unreadable, and this suite runs inside the gate.
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

/// ⚠ **THE SECOND START IS THE ONE THAT FAILED.** `work_dir` is a k8s emptyDir:
/// wiped when the pod goes, kept when the container merely restarts. The key is
/// written and then tightened to 0400, so a restart met a file it owned and could
/// not open for writing — EACCES — and nothing in the pod ever cleared it.
///
/// Measured on isis: one restart at 2026-09-04T08:19Z, then 244 identical
/// failures over 26 hours with the archive answering 502 throughout.
///
/// Twice is the whole test. Once always worked.
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

    // The same process, the same directory, the file already there at 0400.
    assert!(
        IrcSender::prepare(&cfg).await.unwrap().is_some(),
        "a restart must not be fatal — this is the 26-hour outage"
    );

    // And it staged the key again rather than merely tolerating the old one:
    // a stale key from a rotated secret would authenticate as the wrong host.
    assert_eq!(
        std::fs::read(&staged).unwrap(),
        std::fs::read(keys.join("id_ed25519")).unwrap()
    );
}

/// The permission the whole dance exists for: ssh refuses a private key that
/// carries any group or other bit, whatever the secret volume presented.
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

/// A key that is not mounted disables sending; it does not stop the archive.
/// The counterpart in `main.rs` is that a key that cannot be STAGED must answer
/// the same way — that one is not reachable from here, because making the
/// scratch directory unwritable to its owner requires being someone else.
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
