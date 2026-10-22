//! Pictures for the links in a page's messages: held ones are attached, the
//! rest offered.

use anyhow::{Context, Result, bail};
use sqlx::{AssertSqlSafe, MySqlPool, Row};

use crate::link_image::LinkState;

use super::{LinkImage, LinkOffer, Message, bind_all, distinct, in_list};

/// Hang any pictures we hold onto the messages whose text linked them.
///
/// One query for the page, over the links in the bodies already fetched.
pub async fn attach(pool: &MySqlPool, msgs: &mut [Message]) -> Result<()> {
    let mut wanted: Vec<(String, String, usize)> = Vec::new(); // (hash, url, message index)
    for (i, m) in msgs.iter().enumerate() {
        let Some(body) = m.body.as_deref() else {
            continue;
        };
        for url in crate::link_image::urls_in(body) {
            wanted.push((crate::link_fetch::url_hash(&url), url.to_string(), i));
        }
    }
    if wanted.is_empty() {
        return Ok(());
    }
    let hashes = distinct(wanted.iter().map(|(h, _, _)| h.as_str()));
    let sql = in_list(
        "SELECT url_hash, state, content_type FROM link_images WHERE url_hash IN ({in})",
        hashes.len(),
    );
    let mut known: Vec<(String, LinkState, Option<String>)> = Vec::new();
    for row in bind_all(sqlx::query(AssertSqlSafe(sql)), &hashes)
        .fetch_all(pool)
        .await?
    {
        let raw: String = row.try_get("state")?;
        let Some(state) = LinkState::parse(&raw) else {
            bail!("link_images.state holds {raw:?}, which is not a state this app writes");
        };
        known.push((
            row.try_get("url_hash")?,
            state,
            row.try_get("content_type")?,
        ));
    }

    // Per link: a picture we hold is rendered; any other is offered.
    let mut unseen: Vec<(&str, &str)> = Vec::new();
    for (hash, url, i) in &wanted {
        let offered = match known.iter().find(|(h, _, _)| h == hash) {
            Some((_, LinkState::Ok, Some(content_type))) => {
                msgs[*i].link_images.push(LinkImage {
                    url: url.clone(),
                    id: hash.clone(),
                    content_type: content_type.clone(),
                });
                false
            }
            // Held, but with no type to serve it as: neither drawn nor asked for.
            Some((_, LinkState::Ok, None)) => false,
            Some((_, LinkState::Offered, _)) => true,
            None => {
                if !unseen.iter().any(|(h, _)| h == hash) {
                    unseen.push((hash.as_str(), url.as_str()));
                }
                true
            }
        };
        if offered && !msgs[*i].link_offers.iter().any(|o| o.id == *hash) {
            msgs[*i].link_offers.push(LinkOffer {
                id: hash.clone(),
                url: url.clone(),
            });
        }
    }
    offer(pool, &unseen).await?;
    Ok(())
}

/// Register this page's links so they can be asked for later. This is the only
/// place a URL enters `link_images`, and it comes from a message, never a
/// request, so a tap cannot make this an open proxy. `INSERT IGNORE` leaves an
/// existing row, a held picture included, untouched.
async fn offer(pool: &MySqlPool, links: &[(&str, &str)]) -> Result<()> {
    if links.is_empty() {
        return Ok(());
    }
    let rows = vec!["(?, ?, 'offered', NOW())"; links.len()].join(",");
    let sql =
        format!("INSERT IGNORE INTO link_images (url_hash, url, state, wanted_at) VALUES {rows}");
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for (hash, url) in links {
        q = q.bind(*hash).bind(*url);
    }
    q.execute(pool).await.context("offering links")?;
    Ok(())
}

/// The URL of a link we offered, by its hash, so a request never names an
/// address. None once the picture is held.
pub async fn offered_url(pool: &MySqlPool, id: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT url FROM link_images WHERE url_hash = ? AND state = 'offered'")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// Where a link's stored bytes are, if we hold them.
pub async fn blob(pool: &MySqlPool, id: &str) -> Result<Option<(String, String)>> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT content_type, stored_name FROM link_images WHERE url_hash = ? AND state = 'ok'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some((Some(ct), Some(name))) => Some((ct, name)),
        _ => None,
    })
}
