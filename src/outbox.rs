//! Waiting for a send to be answered. The viewer takes no connection to Telegram
//! or Signal: it queues a row, and the process that holds the account sends it
//! and answers in the row (archiver migrations v59 and v64 have the states).

use std::time::Duration;

use anyhow::Result;
use sqlx::MySqlPool;

/// How long a reader waits. Well inside each feed's own `OUTBOX_FRESH_SECS`, so
/// a send it has not reached by then is withdrawn first.
const WAIT: Duration = Duration::from_secs(20);

/// How often the row is read while waiting.
const POLL: Duration = Duration::from_millis(250);

/// Whose outbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outbox {
    /// `telegram_outbox`, served by the Telegram feed.
    Telegram,
    /// `signal_outbox`, served by the Signal ingester.
    Signal,
}

impl Outbox {
    /// The process that answers, as the reader is told.
    fn feed(self) -> &'static str {
        match self {
            Outbox::Telegram => "The Telegram feed",
            Outbox::Signal => "The Signal ingester",
        }
    }
}

/// What became of a send.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It went. `archived` is false when the feed could not store it yet;
    /// `note` is what the reader should still know, such as who it missed.
    Sent {
        archived: bool,
        note: Option<String>,
    },
    /// It did not go, and why.
    Refused(String),
    /// The feed took it and has not answered: it may yet go, or have gone.
    Unanswered,
}

/// Wait for row `id` to be answered, withdrawing it if the feed never reached
/// it.
pub async fn wait(pool: &MySqlPool, outbox: Outbox, id: i64) -> Result<Outcome> {
    wait_for(pool, outbox, id, WAIT).await
}

/// [`wait`], for `patience` rather than `WAIT`.
pub async fn wait_for(
    pool: &MySqlPool,
    outbox: Outbox,
    id: i64,
    patience: Duration,
) -> Result<Outcome> {
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        if let Some(outcome) = answer(pool, outbox, id).await? {
            return Ok(outcome);
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    // Conditional, so it loses to a feed that has just claimed it.
    let withdraw = match outbox {
        Outbox::Telegram => sqlx::query(
            "UPDATE telegram_outbox SET state = 'failed', done_at = CURRENT_TIMESTAMP,
                    note = 'the Telegram feed did not answer; not sent'
              WHERE id = ? AND state = 'queued'",
        ),
        Outbox::Signal => sqlx::query(
            "UPDATE signal_outbox SET state = 'failed', done_at = CURRENT_TIMESTAMP,
                    note = 'the Signal ingester did not answer; not sent'
              WHERE id = ? AND state = 'queued'",
        ),
    };
    if withdraw.bind(id).execute(pool).await?.rows_affected() == 1 {
        return Ok(Outcome::Refused(format!(
            "{} did not answer; not sent.",
            outbox.feed()
        )));
    }
    Ok(answer(pool, outbox, id)
        .await?
        .unwrap_or(Outcome::Unanswered))
}

/// The answer for row `id`, or `None` while there is none. Telegram's note on a
/// send says only that it is not archived; Signal's says who it missed, and its
/// `archived` column says the rest.
async fn answer(pool: &MySqlPool, outbox: Outbox, id: i64) -> Result<Option<Outcome>> {
    let (state, note, archived): (String, Option<String>, Option<bool>) = match outbox {
        Outbox::Telegram => {
            sqlx::query_as(
                "SELECT CAST(state AS CHAR), note, CAST(NULL AS SIGNED) FROM telegram_outbox WHERE id = ?",
            )
            .bind(id)
            .fetch_one(pool)
            .await?
        }
        Outbox::Signal => {
            sqlx::query_as("SELECT CAST(state AS CHAR), note, archived FROM signal_outbox WHERE id = ?")
                .bind(id)
                .fetch_one(pool)
                .await?
        }
    };
    Ok(match state.as_str() {
        "sent" => Some(match outbox {
            Outbox::Telegram => Outcome::Sent {
                archived: note.is_none(),
                note: None,
            },
            Outbox::Signal => Outcome::Sent {
                archived: archived == Some(true),
                note,
            },
        }),
        "failed" => Some(Outcome::Refused(
            note.unwrap_or_else(|| "not sent".to_owned()),
        )),
        "queued" | "sending" => None,
        other => anyhow::bail!("{outbox:?} outbox holds an unknown state: {other:?}"),
    })
}
