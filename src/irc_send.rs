//! Sending one IRC message, as Pippijn, through the irssi that already holds his
//! connections.
//!
//! ⚠ **THIS IS THE ONLY THING THE APP DOES THAT ANOTHER PERSON SEES.** Everything
//! else here is a read over an archive of conversations that already happened.
//! What keeps that from being alarming is that almost none of the safety lives
//! in this file:
//!
//!   * the app's own gate is the Nextcloud login plus the `pippijn`-only
//!     allow-list, which is what already guards the archive;
//!   * the key this presents is pinned on the far side to
//!     `command="/home/irssi/bin/irc-send",restrict`, so a stolen key can send
//!     an IRC message and cannot get a shell, read the logs, or forward a port;
//!   * **who may be messaged is decided on the irssi host**, and by the live
//!     state rather than a list: the plugin asks irssi whether it has a window
//!     item open for the target, so what this app can say something to is
//!     exactly what Pippijn could have typed into. A compromised app cannot
//!     message a stranger, because it is not the thing that decides — and the
//!     thing that decides cannot go stale, because it is not a description of
//!     the conversations he is in, it *is* them.
//!
//! So this module's job is narrow: hand a request over, and turn the answer into
//! a row. It deliberately does not validate the message — the plugin does, and
//! two places that decide would drift.
//!
//! ## Why the echo is inserted here rather than waited for
//!
//! The message lands in irssi's own log, so the hourly importer picks it up like
//! any other line and the archive needs no special case. But hourly is not a
//! chat: the phone is the whole point of this feature, and a message you cannot
//! see for an hour reads as a failure. So the plugin reports what irssi logged —
//! the line, its number, the tag and the nick — and that one row is written on
//! exactly the importer's dedupe key, `(conversation, source_tag, file_date,
//! line_no)`. The next import then finds it already present.
//!
//! ⚠ The fields that make up that key come from **irssi**, not from this app's
//! belief about what it sent. Guessing the tag or the line number and getting it
//! wrong would not lose the message — it would show it twice.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sqlx::MySqlPool;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::IrcSend;

/// irssi is single-threaded: a wedged one must not hold a request open for ever.
/// Generous next to a send that normally answers in well under a second.
const TIMEOUT: Duration = Duration::from_secs(20);

/// A reply larger than this is not something this protocol produces.
const MAX_REPLY: usize = 64 * 1024;

pub struct IrcSender {
    host: String,
    port: u16,
    /// The 0400 copy, in the writable scratch mount.
    key: PathBuf,
    known_hosts: PathBuf,
}

/// What irssi says happened. `Refused` is a normal outcome, not an error: the
/// far side declining to message somebody it has no tab open with is the rule
/// working, not a fault to report as one.
#[derive(Debug)]
pub enum Outcome {
    Sent(Sent),
    Refused(String),
}

/// What a leading slash means: the action, the escape, and nothing else.
///
/// ⚠ **THIS EXISTS BECAUSE `/me` WENT OUT AS FOUR LITERAL CHARACTERS.** The send
/// path hands the composer's text to irssi as DATA that never reaches a command
/// parser — that is what makes `/exec` and an embedded newline harmless from a
/// web request, and it is not up for negotiation. The cost was that `/me`, which
/// is not really a command but content, was transmitted verbatim to a channel.
///
/// So exactly one command is understood, and it is turned into a FLAG rather
/// than into anything a parser sees:
///
///   * `/me waves` → an action saying `waves`;
///   * `//anything` → the literal text `/anything`, IRC's own escape;
///   * anything else, including `/quit` and `/usr/bin/foo`, is unchanged and
///     sent as text.
///
/// ⚠ Other slash words are deliberately NOT refused. They are harmless — they
/// reach no parser — and refusing them would break the ordinary case of a
/// message that starts with a path, which in `#linux` is not hypothetical.
pub fn parse_slash(text: &str) -> (&str, bool) {
    if let Some(rest) = text.strip_prefix("/me ")
        && !rest.trim().is_empty()
    {
        return (rest, true);
    }
    if let Some(rest) = text.strip_prefix('/')
        && rest.starts_with('/')
    {
        return (rest, false);
    }
    (text, false)
}

/// A message irssi has put on the wire, described by irssi.
#[derive(Debug)]
pub struct Sent {
    /// irssi's server tag, which is the directory it logs under and therefore
    /// what the importer stores as `source_tag`.
    pub tag: String,
    /// Who the line is attributed to — the server's *current* nick, which is not
    /// always the configured one after a collision on connect.
    pub nick: String,
    pub text: String,
    /// Whether this went out as an action (`/me`). Carried through because the
    /// archive stores it as a different `kind`, and the row this app writes has
    /// to be the row the importer would write.
    pub is_action: bool,
    /// Absent when the send worked but the echo was not found in the log. That
    /// is not a failure: the message has gone, and the hourly import will still
    /// pick it up. It only means this app cannot show it immediately.
    pub logged: Option<Logged>,
}

#[derive(Debug)]
pub struct Logged {
    pub file_date: String,
    pub line_no: u32,
    /// The raw log line, whose leading `HH:MM` is the only timestamp irssi
    /// records — the date comes from the path.
    pub line: String,
}

/// The plugin's wire format. Mirrors `archive-send.pl`.
#[derive(Deserialize)]
struct Reply {
    ok: bool,
    error: Option<String>,
    tag: Option<String>,
    nick: Option<String>,
    text: Option<String>,
    logged: Option<bool>,
    file_date: Option<String>,
    line_no: Option<u32>,
    line: Option<String>,
}

impl IrcSender {
    /// Copy the key somewhere ssh will accept it, or report that sending is off.
    ///
    /// ⚠ **0400 IS NOT ACHIEVABLE ON THE SECRET VOLUME ITSELF**, which is why
    /// this copy exists. A secret volume's files are owned by *root* rather than
    /// by `runAsUser`, so mounting at 0400 makes them unreadable by this pod —
    /// and it does not surface as a permissions error: ssh reports "No ED25519
    /// host key is known", because an unreadable `known_hosts` is
    /// indistinguishable from an empty one. The volume is mounted 0444 and the
    /// key tightened here, because ssh refuses a key with any group or other bit
    /// whatever the volume says.
    ///
    /// A missing key is `Ok(None)`, not an error: the app must still serve the
    /// archive when it cannot send.
    pub async fn prepare(cfg: &IrcSend) -> Result<Option<Self>> {
        let src = Path::new(&cfg.key_dir).join("id_ed25519");
        let known_hosts = Path::new(&cfg.key_dir).join("known_hosts");
        if !src.exists() {
            tracing::info!(
                "no IRC send key at {}; sending is disabled and the archive is read-only",
                src.display()
            );
            return Ok(None);
        }
        if !known_hosts.exists() {
            bail!(
                "IRC send key present at {} but no known_hosts beside it; \
                 refusing to send to an unverified host",
                src.display()
            );
        }

        let key = Path::new(&cfg.work_dir).join("id_ed25519");
        let bytes = tokio::fs::read(&src)
            .await
            .with_context(|| format!("reading {}", src.display()))?;
        tokio::fs::write(&key, &bytes)
            .await
            .with_context(|| format!("writing {}", key.display()))?;
        set_owner_only(&key).await?;

        Ok(Some(Self {
            host: cfg.host.clone(),
            port: cfg.port,
            key,
            known_hosts,
        }))
    }

    /// Ask irssi to say `text` to `target` on `network`, as a message or as an
    /// action (`/me`).
    ///
    /// ⚠ **No shell, and no command line either.** `ssh host cmd args` is not an
    /// exec: ssh joins its arguments with spaces and the far side's shell splits
    /// and expands them again. Here there is nothing to get wrong, because the
    /// request travels on **stdin** as one JSON line and no command is proposed
    /// at all — the far side runs its forced command regardless.
    pub async fn send(
        &self,
        network: &str,
        target: &str,
        text: &str,
        is_action: bool,
    ) -> Result<Outcome> {
        // ⚠ A FLAG, and the CTCP framing an action needs is built by the PLUGIN
        // rather than sent from here. `\x01ACTION …\x01` is control characters,
        // which the plugin refuses on principle — CR and LF end a protocol line
        // — and carving an exception for one of them would restore exactly the
        // allow-list judgement the design removed.
        let request = serde_json::json!({
            "network": network,
            "target": target,
            "text": text,
            "action": is_action,
        })
        .to_string();

        let mut child = Command::new("ssh")
            .arg("-T")
            .arg("-q")
            // No user config at all. The pod has no home directory to speak of,
            // and an ssh that reads one would be taking instructions from
            // whatever happened to be in the image rather than from here.
            .args(["-F", "/dev/null"])
            .args(["-o", "BatchMode=yes"])
            // Only the key we were given: without this, ssh would offer any
            // agent or default identity first and could authenticate as
            // something else entirely.
            .args(["-o", "IdentitiesOnly=yes"])
            .args(["-o", "StrictHostKeyChecking=yes"])
            .arg("-o")
            .arg(format!("UserKnownHostsFile={}", self.known_hosts.display()))
            .args(["-o", "ConnectTimeout=10"])
            .arg("-i")
            .arg(&self.key)
            .arg("-p")
            .arg(self.port.to_string())
            .arg(format!("irssi@{}", self.host))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // So the timeout below actually ends the process. Dropping a child
            // without this leaves ssh running and the pipe open, and the read
            // that was supposed to time out never returns.
            .kill_on_drop(true)
            .spawn()
            .context("spawning ssh")?;

        let mut stdin = child.stdin.take().context("ssh stdin was not piped")?;
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .context("writing the request to ssh")?;
        // EOF, so the far side stops waiting for more of the request.
        stdin.shutdown().await.context("closing ssh stdin")?;
        drop(stdin);

        let out = tokio::time::timeout(TIMEOUT, child.wait_with_output())
            .await
            .context("irssi did not answer in time")?
            .context("waiting for ssh")?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!(
                "could not reach irssi (ssh exited {}): {}",
                out.status.code().unwrap_or(-1),
                stderr.trim()
            );
        }
        if out.stdout.len() > MAX_REPLY {
            bail!("irssi's answer was implausibly large");
        }

        let reply: Reply = serde_json::from_slice(&out.stdout)
            .context("irssi's answer was not the expected JSON")?;

        if !reply.ok {
            return Ok(Outcome::Refused(
                reply.error.unwrap_or_else(|| "refused".to_string()),
            ));
        }

        let logged = match (reply.logged, reply.file_date, reply.line_no, reply.line) {
            (Some(true), Some(file_date), Some(line_no), Some(line)) => Some(Logged {
                file_date,
                line_no,
                line,
            }),
            _ => None,
        };

        Ok(Outcome::Sent(Sent {
            tag: reply.tag.context("irssi did not report its server tag")?,
            nick: reply.nick.context("irssi did not report its nick")?,
            text: reply.text.unwrap_or_else(|| text.to_string()),
            is_action,
            logged,
        }))
    }
}

#[cfg(unix)]
async fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400))
        .await
        .with_context(|| format!("tightening {} to 0400", path.display()))
}

/// irssi records `%H:%M` and nothing finer, so the seconds are always zero and
/// the date comes from the file's path. Reading the clock here instead would
/// disagree with the importer for the same line whenever a send straddles a
/// minute — and `sent_at` is what the reader orders by.
fn sent_at(file_date: &str, line: &str) -> Option<String> {
    let (hh, rest) = line.split_once(':')?;
    let mm = rest.get(..2)?;
    if hh.len() != 2 || !hh.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !mm.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("{file_date} {hh}:{mm}:00"))
}

/// Write the sent message into the archive so it can be shown at once.
///
/// ⚠ **`INSERT IGNORE` on the importer's key, and both halves matter.** Ignore,
/// because the hourly import will read the very same line and must not fail on
/// it; and the importer's exact key, because a row keyed differently is not a
/// duplicate to the database — it is a second message, and the conversation
/// shows it twice.
///
/// Returns whether a row was actually written. `false` means the importer got
/// there first, which is a race with no consequence.
pub async fn record_echo(pool: &MySqlPool, conversation_id: &str, sent: &Sent) -> Result<bool> {
    let Some(logged) = &sent.logged else {
        return Ok(false);
    };
    let Some(sent_at) = sent_at(&logged.file_date, &logged.line) else {
        // The line is not shaped like a log line, so the timestamp cannot be
        // taken from it. Writing a guessed one would put the message in the
        // wrong place in the conversation; leaving it to the import puts it in
        // the right place, late.
        tracing::warn!(
            "irssi's echo did not start with a timestamp, leaving it to the importer: {}",
            logged.line
        );
        return Ok(false);
    };

    let res = sqlx::query(
        r"INSERT IGNORE INTO irc_messages
            (conversation_id, source_tag, file_date, line_no, sent_at, nick, is_self, kind, text)
          VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)",
    )
    .bind(conversation_id)
    .bind(&sent.tag)
    .bind(&logged.file_date)
    .bind(logged.line_no)
    .bind(&sent_at)
    .bind(&sent.nick)
    // ⚠ The KIND the importer would give this line, not 'message' always. An
    // action logged as ` * nick waves` parses as `Kind::Action`; writing the
    // echo as a message would put a row on the dedupe key that disagrees with
    // the file, and `INSERT IGNORE` means the reconciler can never correct it.
    .bind(if sent.is_action { "action" } else { "message" })
    .bind(&sent.text)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}
