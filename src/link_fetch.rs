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

use crate::link_image::read_advert;

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

/// Ask the fetch service for a picture, and write down what came back.
///
/// ⚠ **THE WEB POD DOES NOT FETCH, AND THE FETCHER DOES NOT READ.** This side
/// holds the archive's credentials and the stored pictures; the other side holds
/// a socket to the internet and nothing else. The bytes come back over one
/// in-cluster request, so a fetcher that has been talked into something by a
/// hostile page has no database to read, no volume to write and no credential to
/// steal — the worst it can do is lie about the bytes of a picture somebody
/// asked for, which is the same thing the remote server could have done anyway.
pub async fn resolve_one(
    pool: &MySqlPool,
    client: &reqwest::Client,
    service: &str,
    dir: &Path,
    url: &Url,
) -> Result<Outcome> {
    match ask_service(client, service, url).await {
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
            let note = e.to_string();
            record(pool, url, Outcome::Failed, None, None, None, Some(&note)).await?;
            Ok(Outcome::Failed)
        }
    }
}

/// One in-cluster request. 204 means "reached it, not a picture"; a 5xx carries
/// the reason, which is kept because "failed" with no reason is the kind of row
/// that gets retried by hand for ever.
async fn ask_service(
    client: &reqwest::Client,
    service: &str,
    url: &Url,
) -> Result<Option<(Vec<u8>, String)>> {
    let res = client
        .post(format!("{service}/fetch"))
        .json(&serde_json::json!({ "url": url.as_str() }))
        .send()
        .await
        .context("asking the link fetcher")?;
    if res.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(None);
    }
    if !res.status().is_success() {
        let status = res.status();
        let why = res.text().await.unwrap_or_default();
        anyhow::bail!(
            "fetcher said {status}: {}",
            why.chars().take(200).collect::<String>()
        );
    }
    let content_type = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_owned())
        .unwrap_or_default();
    if !content_type.starts_with("image/") {
        return Ok(None);
    }
    Ok(Some((res.bytes().await?.to_vec(), content_type)))
}

/// Two GETs at most: the page, then the picture the page named. Returns None when
/// the link is simply not an inlineable picture, which is not an error.
pub async fn fetch_picture(
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
              (url_hash, url, state, content_type, size_bytes, stored_name, note,
               wanted_at, fetched_at)
          VALUES (?, ?, ?, ?, ?, ?, ?, NOW(), NOW())
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
