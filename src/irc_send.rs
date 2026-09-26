//! Sending one IRC message, as Pippijn, through the irssi that holds his
//! connections.
//!
//! The safety is mostly elsewhere: the app's Nextcloud login and `pippijn`-only
//! allow-list; the key's forced command on the far side
//! (`command="/home/irssi/bin/irc-send",restrict`); and the plugin, which sends
//! only to targets irssi has a window open for. So this module hands a request
//! over and records the answer, without validating the message itself.
//!
//! The echo is written at once, so the phone shows it without waiting for the
//! hourly import: the plugin reports what irssi logged (line, number, tag,
//! nick), and the line is parsed into a row by the importer's own `irclog`, on
//! its dedupe key `(conversation, source_tag, file_date, line_no)`, which the
//! import then finds present.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sqlx::MySqlPool;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::IrcSend;

/// irssi is single-threaded; a wedged one must not hold a request forever.
const TIMEOUT: Duration = Duration::from_secs(20);

/// No reply this protocol produces is larger.
const MAX_REPLY: usize = 64 * 1024;

pub struct IrcSender {
    host: String,
    port: u16,
    /// The 0400 copy, in the writable scratch mount.
    key: PathBuf,
    known_hosts: PathBuf,
}

/// What irssi says happened. `Refused` is a normal outcome: the target has no
/// window open.
#[derive(Debug)]
pub enum Outcome {
    Sent(Sent),
    Refused(String),
}

/// What a leading slash means. The text reaches irssi as data, never through a
/// command parser, so one command is recognised here and turned into a flag:
///
///   * `/me waves` → an action saying `waves`;
///   * `//anything` → the literal text `/anything`, IRC's escape;
///   * anything else, `/quit` and `/usr/bin/foo` included, is sent as text.
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
    /// irssi's server tag: the directory it logs under, the importer's
    /// `source_tag`.
    pub tag: String,
    /// The server's current nick, which can differ from the configured one.
    pub nick: String,
    /// `None` when the send worked but the echo was not found in the log; the
    /// import will pick it up.
    pub logged: Option<Logged>,
}

#[derive(Debug)]
pub struct Logged {
    pub file_date: String,
    pub line_no: u32,
    /// The raw log line; its leading `HH:MM` is irssi's only timestamp.
    pub line: String,
}

/// The plugin's wire format, the fields this reads; see `archive-send.pl`.
#[derive(Deserialize)]
struct Reply {
    ok: bool,
    error: Option<String>,
    tag: Option<String>,
    nick: Option<String>,
    logged: Option<bool>,
    file_date: Option<String>,
    line_no: Option<u32>,
    line: Option<String>,
}

impl IrcSender {
    /// Copy the key where ssh accepts it, or report that sending is off.
    ///
    /// A secret volume's files are root-owned, so a 0400 mount is unreadable by
    /// this pod (and ssh reports that as an unknown host key). The volume is
    /// mounted 0444 and this copy tightened, since ssh refuses group or other bits.
    ///
    /// No key is `Ok(None)`: the archive is still served.
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

        // The work dir is an emptyDir, which survives a container restart, and
        // the copy is 0400, so a restart cannot overwrite it. Remove it first.
        match tokio::fs::remove_file(&key).await {
            Ok(()) => tracing::info!("cleared {} left by an earlier start", key.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| format!("clearing {}", key.display()));
            }
        }

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
    /// The request travels on stdin as one JSON line; the far side runs its
    /// forced command, so no command line is proposed.
    pub async fn send(
        &self,
        network: &str,
        target: &str,
        text: &str,
        is_action: bool,
    ) -> Result<Outcome> {
        // A flag: the plugin builds the CTCP action framing, since it refuses
        // control characters in text.
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
            // No user config.
            .args(["-F", "/dev/null"])
            .args(["-o", "BatchMode=yes"])
            // Only this key, never an agent's or a default identity.
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
            // So the timeout below ends ssh rather than leaving it running.
            .kill_on_drop(true)
            .spawn()
            .context("spawning ssh")?;

        let mut stdin = child.stdin.take().context("ssh stdin was not piped")?;
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .context("writing the request to ssh")?;
        // EOF ends the request.
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

/// Write the sent message into the archive so it can be shown at once.
///
/// `INSERT IGNORE` on the importer's exact key, so the import's later write of
/// the same line is the same row. Returns whether a row was written.
pub async fn record_echo(pool: &MySqlPool, conversation_id: &str, sent: &Sent) -> Result<bool> {
    let Some(logged) = &sent.logged else {
        return Ok(false);
    };
    // The importer's parser and row, so the two writes are one row.
    let entry = irclog::Date::parse_iso(&logged.file_date)
        .map(|date| irclog::parse_log(date, &format!("{}\n", logged.line)))
        .and_then(|parsed| parsed.entries.into_iter().next());
    let Some(entry) = entry else {
        // Not a log line: leave it to the import rather than guess.
        tracing::warn!(
            "irssi's echo is not a log line, leaving it to the importer: {} {}",
            logged.file_date,
            logged.line
        );
        return Ok(false);
    };
    let row = irclog::IrcLine::from_entry(&entry, logged.line_no, std::slice::from_ref(&sent.nick));

    let res = sqlx::query(
        r"INSERT IGNORE INTO irc_messages
            (conversation_id, source_tag, file_date, line_no, sent_at, nick, is_self, kind, text)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(conversation_id)
    .bind(&sent.tag)
    .bind(&logged.file_date)
    .bind(row.line_no)
    .bind(&row.sent_at)
    .bind(&row.nick)
    .bind(row.is_self)
    .bind(row.kind.as_str())
    .bind(&row.text)
    .execute(pool)
    .await?;

    Ok(res.rows_affected() > 0)
}
