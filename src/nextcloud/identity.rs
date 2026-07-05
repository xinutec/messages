//! Nextcloud OAuth2 — identity only.
//!
//! Establishes *who the user is* and nothing else. The access token is used
//! once to read `{id, displayname}` then discarded; no refresh token is stored.
//! Copied from the `life` app, which is the reference implementation.

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

use crate::config::Config;

/// Build the URL the browser is redirected to in order to grant access.
pub fn authorize_url(cfg: &Config, state: &str) -> String {
    let mut url = url::Url::parse(&format!(
        "{}/index.php/apps/oauth2/authorize",
        cfg.nc_base_url
    ))
    .expect("nc_base_url validated at config load");
    url.query_pairs_mut()
        .append_pair("client_id", &cfg.nc_client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &cfg.nc_redirect_uri)
        .append_pair("state", state);
    url.to_string()
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// Exchange an authorization `code` for an access token.
pub async fn exchange_code(http: &reqwest::Client, cfg: &Config, code: &str) -> Result<String> {
    let res = http
        .post(format!(
            "{}/index.php/apps/oauth2/api/v1/token",
            cfg.nc_base_url
        ))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", cfg.nc_client_id.as_str()),
            ("client_secret", cfg.nc_client_secret.as_str()),
            ("redirect_uri", cfg.nc_redirect_uri.as_str()),
        ])
        .send()
        .await?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(anyhow!("NC token exchange failed: {status}: {body}"));
    }
    let token: TokenResponse = res.json().await.context("parsing NC token response")?;
    Ok(token.access_token)
}

pub struct NcUser {
    pub id: String,
    pub display_name: String,
}

#[derive(Deserialize)]
struct OcsEnvelope {
    ocs: OcsBody,
}
#[derive(Deserialize)]
struct OcsBody {
    data: OcsData,
}
#[derive(Deserialize)]
struct OcsData {
    id: String,
    displayname: String,
}

/// Look up the granting user's id + display name. The token is consumed here.
pub async fn fetch_user(
    http: &reqwest::Client,
    cfg: &Config,
    access_token: &str,
) -> Result<NcUser> {
    let res = http
        .get(format!(
            "{}/ocs/v2.php/cloud/user?format=json",
            cfg.nc_base_url
        ))
        .header("Authorization", format!("Bearer {access_token}"))
        .header("OCS-APIRequest", "true")
        .send()
        .await?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(anyhow!("NC user info failed: {status}: {body}"));
    }
    let parsed: OcsEnvelope = res.json().await.context("parsing NC user info")?;
    Ok(NcUser {
        id: parsed.ocs.data.id,
        display_name: parsed.ocs.data.displayname,
    })
}
