//! The web pod's half of fetching a link's picture: ask the fetch service, then
//! store the bytes. Only a picture is written down; "not a picture" and "could
//! not reach it" go back to the reader and to the log, so the link can be asked
//! again once the page or this code has changed.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use sqlx::MySqlPool;
use url::Url;

use crate::link_image::read_advert;

/// What a fetch may cost: the point past which we stop reading.
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

/// How one request for a link's picture ended. Only `Ok` is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, rename = "LinkOutcome"))]
pub enum Outcome {
    /// Bytes on the volume, servable.
    Ok,
    /// Reached it, and it is not a picture we may inline, today.
    NotImage,
    /// Could not reach it, or it broke the limits.
    Failed,
}

/// A link's identity on disk and in the table: the URL's SHA-256. One row and
/// one file per link, and the name reveals nothing about its source.
pub fn url_hash(url: &Url) -> String {
    hex::encode(Sha256::digest(url.as_str().as_bytes()))
}

/// Ask the fetch service for a picture, and store it if one came back.
///
/// The fetch service reaches the internet and holds nothing; this side holds the
/// credentials and the volume. A compromised fetcher can at worst lie about a
/// picture's bytes.
pub async fn resolve_one(
    pool: &MySqlPool,
    client: &reqwest::Client,
    service: &str,
    dir: &Path,
    url: &Url,
) -> Result<(Outcome, Option<String>)> {
    match ask_service(client, service, url).await {
        Ok(Some((bytes, content_type))) => {
            let name = store(dir, url, &content_type, &bytes).await?;
            record_held(pool, url, &content_type, bytes.len(), &name).await?;
            Ok((Outcome::Ok, Some(content_type)))
        }
        Ok(None) => {
            tracing::info!(link = %url_hash(url), "not a picture");
            Ok((Outcome::NotImage, None))
        }
        Err(e) => {
            tracing::warn!(link = %url_hash(url), "could not fetch the picture: {e:#}");
            Ok((Outcome::Failed, None))
        }
    }
}

/// One in-cluster request. 204 means not a picture; a 5xx carries the reason,
/// which is logged.
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
    let content_type = header(&res, reqwest::header::CONTENT_TYPE).unwrap_or_default();
    if !content_type.starts_with("image/") {
        return Ok(None);
    }
    // Capped here too: this side holds the credentials, and the fetcher's word
    // about size is not to be trusted.
    Ok(Some((
        read_capped(res, Limits::default()).await?,
        content_type,
    )))
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
    let final_url = res.url().clone();
    match header(&res, reqwest::header::CONTENT_TYPE) {
        // A link straight to a picture needs no page.
        Some(ct) if ct.starts_with("image/") => {
            return Ok(Some((read_capped(res, limits).await?, ct)));
        }
        Some(ct) if ct.starts_with("text/html") => {}
        _ => return Ok(None),
    }

    let cookies: Vec<String> = res
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    let html = String::from_utf8_lossy(&read_capped(res, limits).await?).into_owned();

    let advert = read_advert(&final_url, cookies.iter().map(String::as_str), &html);
    if let Some(why) = advert.refusal() {
        tracing::info!("not a picture: {why}");
        return Ok(None);
    }
    let Some(image_url) = advert.image else {
        return Ok(None);
    };

    let res = client.get(image_url).timeout(limits.timeout).send().await?;
    let ct = header(&res, reqwest::header::CONTENT_TYPE).unwrap_or_default();
    // Check the image itself: the file behind a share can change.
    if !ct.starts_with("image/") {
        // A URL gone wrong answers with an error page, which lands here.
        tracing::info!("the advertised picture answered {ct}, not an image");
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

/// Write the bytes under the URL's hash, with an extension from the served type.
async fn store(dir: &Path, url: &Url, content_type: &str, bytes: &[u8]) -> Result<String> {
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
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("creating {}", dir.display()))?;
    let path: PathBuf = dir.join(&name);
    tokio::fs::write(&path, bytes)
        .await
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(name)
}

async fn record_held(
    pool: &MySqlPool,
    url: &Url,
    content_type: &str,
    size: usize,
    stored_name: &str,
) -> Result<()> {
    sqlx::query(
        r"INSERT INTO link_images
              (url_hash, url, state, content_type, size_bytes, stored_name, wanted_at, fetched_at)
          VALUES (?, ?, 'ok', ?, ?, ?, NOW(), NOW())
          ON DUPLICATE KEY UPDATE
              state = 'ok', content_type = VALUES(content_type),
              size_bytes = VALUES(size_bytes), stored_name = VALUES(stored_name),
              fetched_at = NOW()",
    )
    .bind(url_hash(url))
    .bind(url.as_str())
    .bind(content_type)
    .bind(i64::try_from(size)?)
    .bind(stored_name)
    .execute(pool)
    .await
    .context("recording a held picture")?;
    Ok(())
}
