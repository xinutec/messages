//! The job half: find links in the archive, ask each server once what its link
//! is, keep the pictures.
//!
//! ⚠ **THIS RUNS NOWHERE NEAR THE WEB POD.** The pod that serves
//! `messages.xinutec.org` has no route off the cluster at all — DNS, the
//! database, and irssi — and that is deliberate. Fetching a stranger's URL is
//! the one thing in this app that must reach the open internet, so it lives in a
//! scheduled job with its own egress, and the result reaches the reader as bytes
//! on a volume, exactly like a Signal attachment.
//!
//! Politeness is a design constraint, not a nicety: these are other people's
//! servers, and the links are years old. Every decision is written down —
//! including "not a picture" — so each link is asked about ONCE, ever.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use sqlx::MySqlPool;
use url::Url;

use crate::link_image::{read_advert, urls_in};

/// What a fetch is allowed to cost. A share page is ~30 KB and a preview a few
/// hundred; the cap is not a guess about pictures, it is the point past which we
/// stop reading whatever we are being sent.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: usize,
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 12 * 1024 * 1024,
            timeout: Duration::from_secs(20),
        }
    }
}

/// How a link ended up. Every one of these is stored; see the module note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Bytes on the volume, servable.
    Ok,
    /// Reached it, and it is not a picture we may inline. Never asked again.
    NotImage,
    /// Could not reach it, or it broke the limits. Also not asked again — a link
    /// from 2014 that times out today will time out next week too.
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::NotImage => "not_image",
            Outcome::Failed => "failed",
        }
    }
}

/// A link's identity on disk and in the table: the SHA-256 of the URL itself.
/// The same picture posted in three channels is one row and one file, and the
/// name gives away nothing about where it came from.
pub fn url_hash(url: &Url) -> String {
    hex::encode(Sha256::digest(url.as_str().as_bytes()))
}

/// A line of the archive to examine for links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub id: i64,
    pub text: String,
}

/// How far the fetcher has examined the archive.
///
/// ⚠ **TWO WATERMARKS, AND ONE IS NOT ENOUGH.** The archive grows at the top and
/// is inexhaustible at the bottom, so "where have we got to" has two answers:
///
///   * `high` — the newest line examined. Above it are arrivals since the last
///     run, which should be decided promptly; a picture posted this morning
///     should not wait for a walk through 2014.
///   * `low` — the oldest line examined. Below it is history nobody has looked
///     at, walked backwards a chunk per run until it reaches the beginning.
///
/// **The invariant is that every line between `low` and `high` has been
/// examined.** Everything else follows from keeping that true — in particular a
/// watermark may only move over lines whose links were all attempted, which is
/// what `plan` computes and why it refuses to stop in the middle of a line.
///
/// The first run starts them both above the newest id, so the forward pass finds
/// nothing and the backfill begins at the top and walks down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub high: i64,
    pub low: i64,
}

/// What one pass decided to do: the links to attempt, and how far it may claim
/// to have examined.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub urls: Vec<Url>,
    /// The id of the last line examined IN FULL, or None if the budget ran out
    /// before a single line was finished. The watermark moves to exactly here.
    pub examined_through: Option<i64>,
}

/// Decide what to attempt from a chunk of lines, in the order given.
///
/// ⚠ **STOPS BEFORE A LINE IT CANNOT FINISH, and that is the whole reason this
/// is a function rather than a loop in `run`.** A line carrying three links when
/// only one fits in the batch must be left for the next run entirely: taking one
/// link and moving the watermark past the line would drop the other two
/// silently, for ever, and nothing downstream would ever notice — the line is
/// behind the watermark, so it is never read again.
///
/// Lines whose links are all decided already cost nothing and are examined, which
/// is what lets the backfill cross years of link-free conversation in one run.
pub fn plan(lines: &[Line], decided: &dyn Fn(&Url) -> bool, budget: usize) -> Plan {
    let mut out = Plan::default();
    for line in lines {
        let fresh: Vec<Url> = urls_in(&line.text)
            .into_iter()
            .filter(|u| !decided(u) && !out.urls.contains(u))
            .collect();
        if out.urls.len() + fresh.len() > budget {
            return out;
        }
        out.urls.extend(fresh);
        out.examined_through = Some(line.id);
    }
    out
}

/// Read the watermarks, starting them above the newest line on first run.
pub async fn progress(pool: &MySqlPool) -> Result<Progress> {
    let row: Option<(i64, i64)> =
        sqlx::query_as("SELECT high_water, low_water FROM link_fetch_progress WHERE id = 1")
            .fetch_optional(pool)
            .await?;
    if let Some((high, low)) = row {
        return Ok(Progress { high, low });
    }
    let newest: Option<i64> = sqlx::query_scalar("SELECT MAX(id) FROM irc_messages")
        .fetch_one(pool)
        .await?;
    // One past the newest: nothing examined yet, and the backfill starts at the top.
    let start = newest.unwrap_or(0) + 1;
    Ok(Progress {
        high: start,
        low: start,
    })
}

async fn save_progress(pool: &MySqlPool, p: Progress) -> Result<()> {
    sqlx::query(
        r"INSERT INTO link_fetch_progress (id, high_water, low_water) VALUES (1, ?, ?)
          ON DUPLICATE KEY UPDATE high_water = VALUES(high_water), low_water = VALUES(low_water)",
    )
    .bind(p.high)
    .bind(p.low)
    .execute(pool)
    .await
    .context("saving link-fetch progress")?;
    Ok(())
}

/// Lines newer than the watermark, oldest first — arrivals since the last run.
async fn lines_above(pool: &MySqlPool, high: i64, chunk: u32) -> Result<Vec<Line>> {
    fetch_lines(pool, "id > ? ORDER BY id ASC", high, chunk).await
}

/// Lines older than the watermark, newest first — the backfill, walking down.
async fn lines_below(pool: &MySqlPool, low: i64, chunk: u32) -> Result<Vec<Line>> {
    fetch_lines(pool, "id < ? ORDER BY id DESC", low, chunk).await
}

async fn fetch_lines(
    pool: &MySqlPool,
    where_order: &str,
    bound: i64,
    chunk: u32,
) -> Result<Vec<Line>> {
    // Fixed template, one bound value — the direction is chosen from the two
    // literals above, never from input. `ORDER BY` cannot be a bound parameter.
    let sql = format!(
        "SELECT id, text FROM irc_messages
         WHERE kind IN ('message','action') AND text LIKE '%http%' AND {where_order} LIMIT ?"
    );
    let rows: Vec<(i64, Option<String>)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(bound)
        .bind(chunk)
        .fetch_all(pool)
        .await
        .context("reading lines to examine")?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, text)| text.map(|text| Line { id, text }))
        .collect())
}

/// Which of these links already have a decision — one query for the chunk.
async fn decided_set(
    pool: &MySqlPool,
    lines: &[Line],
) -> Result<std::collections::HashSet<String>> {
    let mut hashes: Vec<String> = Vec::new();
    for line in lines {
        for url in urls_in(&line.text) {
            let h = url_hash(&url);
            if !hashes.contains(&h) {
                hashes.push(h);
            }
        }
    }
    if hashes.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let placeholders = vec!["?"; hashes.len()].join(",");
    let sql = format!("SELECT url_hash FROM link_images WHERE url_hash IN ({placeholders})");
    let mut q = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql));
    for h in &hashes {
        q = q.bind(h);
    }
    Ok(q.fetch_all(pool).await?.into_iter().collect())
}

/// How many of a chunk were examined: the prefix up to and including the last
/// line `plan` finished. The chunk is read ahead of what a budget can cover, so
/// this is always ≤ its length.
pub fn examined_count(chunk: &[Line], through: Option<i64>) -> usize {
    match through {
        None => 0,
        Some(id) => chunk.iter().position(|l| l.id == id).map_or(0, |i| i + 1),
    }
}

/// `low` once the walk has reached the beginning of the archive. Zero rather
/// than the oldest line's id, so that "finished" is a value and not an inference
/// from a watermark that has stopped moving.
pub const BACKFILL_DONE: i64 = 0;

/// What a run did, for the log line that is the only thing anyone sees.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    pub stored: u32,
    pub not_image: u32,
    pub failed: u32,
    pub examined_new: usize,
    pub examined_old: usize,
    pub progress: Option<Progress>,
}

/// One pass of the fetcher: new arrivals first, then a chunk of backfill.
///
/// ⚠ **NEW ARRIVALS TAKE THE BUDGET FIRST.** A picture posted this morning
/// should appear in the conversation today; history has waited years and can
/// wait another hour. If the forward pass uses the whole budget the backfill
/// simply does not run, and its watermark does not move — which is the honest
/// outcome, not a skipped step.
pub async fn run(
    pool: &MySqlPool,
    client: &reqwest::Client,
    dir: &Path,
    limits: Limits,
    budget: usize,
    chunk: u32,
) -> Result<RunReport> {
    let mut at = progress(pool).await?;
    let mut report = RunReport::default();
    let mut urls: Vec<Url> = Vec::new();

    let above = lines_above(pool, at.high, chunk).await?;
    if !above.is_empty() {
        let decided = decided_set(pool, &above).await?;
        let planned = plan(&above, &|u| decided.contains(&url_hash(u)), budget);
        if let Some(through) = planned.examined_through {
            at.high = through;
        }
        // ⚠ How many were EXAMINED, not how many were read. The chunk is a read
        // ahead; `plan` stops at the last line it could finish, and the rest are
        // left for the next run. Reporting the chunk size said "examined 5000"
        // for a run whose watermark had moved over a few hundred — a number that
        // names one thing and counts another.
        report.examined_new = examined_count(&above, planned.examined_through);
        urls.extend(planned.urls);
    }

    let remaining = budget.saturating_sub(urls.len());
    if remaining > 0 {
        let below = lines_below(pool, at.low, chunk).await?;
        let short_chunk = below.len() < chunk as usize;
        let last_id = below.last().map(|l| l.id);
        if below.is_empty() {
            // Nothing older carries a link at all: the walk is finished, and
            // saying so is what distinguishes "done" from "stuck". Without a
            // terminal value `low` would rest on the oldest link-bearing line for
            // ever and read exactly like a backfill that had stalled.
            at.low = BACKFILL_DONE;
        } else {
            let decided = decided_set(pool, &below).await?;
            let planned = plan(&below, &|u| decided.contains(&url_hash(u)), remaining);
            if let Some(through) = planned.examined_through {
                at.low = through;
                // Reached the end of the archive AND got through the chunk: there
                // is nothing older to come back for.
                if short_chunk && Some(through) == last_id {
                    at.low = BACKFILL_DONE;
                }
            }
            report.examined_old = examined_count(&below, planned.examined_through);
            urls.extend(planned.urls);
        }
    }

    for url in &urls {
        match resolve_one(pool, client, dir, url, limits).await? {
            Outcome::Ok => report.stored += 1,
            Outcome::NotImage => report.not_image += 1,
            Outcome::Failed => report.failed += 1,
        }
    }

    // ⚠ Written AFTER the fetches, so a run that dies half way re-examines those
    // lines next time rather than skipping them. Re-examining is free: every link
    // it meets again already has a decision.
    save_progress(pool, at).await?;
    report.progress = Some(at);
    Ok(report)
}

/// Ask one server about one link, and write down what it said.
pub async fn resolve_one(
    pool: &MySqlPool,
    client: &reqwest::Client,
    dir: &Path,
    url: &Url,
    limits: Limits,
) -> Result<Outcome> {
    match fetch_picture(client, url, limits).await {
        Ok(Some((bytes, content_type))) => {
            let name = store(dir, url, &content_type, &bytes)?;
            record(
                pool,
                url,
                Outcome::Ok,
                Some(&content_type),
                Some(bytes.len()),
                Some(&name),
                None,
            )
            .await?;
            Ok(Outcome::Ok)
        }
        Ok(None) => {
            record(pool, url, Outcome::NotImage, None, None, None, None).await?;
            Ok(Outcome::NotImage)
        }
        Err(e) => {
            // The reason is kept because "failed" with no reason is the kind of
            // row that gets re-tried by hand for ever.
            let note = e.to_string();
            record(pool, url, Outcome::Failed, None, None, None, Some(&note)).await?;
            Ok(Outcome::Failed)
        }
    }
}

/// Two GETs at most: the page, then the picture the page named. Returns None when
/// the link is simply not an inlineable picture, which is not an error.
async fn fetch_picture(
    client: &reqwest::Client,
    url: &Url,
    limits: Limits,
) -> Result<Option<(Vec<u8>, String)>> {
    let res = client
        .get(url.clone())
        .timeout(limits.timeout)
        .send()
        .await?;
    let final_url = Url::parse(res.url().as_str())?;
    let content_type = header(&res, reqwest::header::CONTENT_TYPE);

    // A link straight to a picture is already the answer; no page to read.
    if content_type
        .as_deref()
        .is_some_and(|t| t.starts_with("image/"))
    {
        let ct = content_type.unwrap_or_else(|| "application/octet-stream".into());
        return Ok(Some((read_capped(res, limits).await?, ct)));
    }
    if !content_type
        .as_deref()
        .is_some_and(|t| t.starts_with("text/html"))
    {
        return Ok(None);
    }

    let cookies: Vec<String> = res
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    let html = String::from_utf8_lossy(&read_capped(res, limits).await?).into_owned();

    let advert = read_advert(&final_url, cookies.iter().map(String::as_str), &html);
    if !advert.is_inlineable_image() {
        return Ok(None);
    }
    let Some(image_url) = advert.image else {
        return Ok(None);
    };

    let res = client.get(image_url).timeout(limits.timeout).send().await?;
    let ct = header(&res, reqwest::header::CONTENT_TYPE).unwrap_or_default();
    // ⚠ The page SAID it was a picture; this is the server actually sending one.
    // A share whose file was replaced since it was posted answers with whatever
    // is there now, and that is the byte stream we would be storing.
    if !ct.starts_with("image/") {
        return Ok(None);
    }
    Ok(Some((read_capped(res, limits).await?, ct)))
}

fn header(res: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    res.headers()
        .get(name)?
        .to_str()
        .ok()
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_owned())
}

/// Read a body, stopping if it grows past the cap rather than after.
async fn read_capped(mut res: reqwest::Response, limits: Limits) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(chunk) = res.chunk().await? {
        if out.len() + chunk.len() > limits.max_bytes {
            anyhow::bail!("body exceeds {} bytes", limits.max_bytes);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Write the bytes under the URL's hash. The extension comes from the type the
/// server sent, so the volume is browsable, and nothing from the URL reaches the
/// filesystem.
fn store(dir: &Path, url: &Url, content_type: &str, bytes: &[u8]) -> Result<String> {
    let ext = match content_type {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/svg+xml" => "svg",
        _ => "bin",
    };
    let name = format!("{}.{ext}", url_hash(url));
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path: PathBuf = dir.join(&name);
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(name)
}

async fn record(
    pool: &MySqlPool,
    url: &Url,
    outcome: Outcome,
    content_type: Option<&str>,
    size: Option<usize>,
    stored_name: Option<&str>,
    note: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r"INSERT INTO link_images
              (url_hash, url, state, content_type, size_bytes, stored_name, note, fetched_at)
          VALUES (?, ?, ?, ?, ?, ?, ?, NOW())
          ON DUPLICATE KEY UPDATE
              state = VALUES(state), content_type = VALUES(content_type),
              size_bytes = VALUES(size_bytes), stored_name = VALUES(stored_name),
              note = VALUES(note), fetched_at = NOW()",
    )
    .bind(url_hash(url))
    .bind(url.as_str())
    .bind(outcome.as_str())
    .bind(content_type)
    .bind(size.map(|s| s as i64))
    .bind(stored_name)
    .bind(note.map(|n| n.chars().take(240).collect::<String>()))
    .execute(pool)
    .await
    .context("recording a link's outcome")?;
    Ok(())
}
