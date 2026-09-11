//! Deciding, from a fetched page alone, whether a link in a message is a
//! picture we can serve ourselves.
//!
//! ⚠ **NOTHING HERE KNOWS A HOST, A PATH OR A PRODUCT URL.** The rule is that a
//! server which can be inlined says so itself, in the response:
//!
//!   * it NAMES ITS PRODUCT in the cookies it sets — Nextcloud and ownCloud set
//!     `nc_sameSiteCookie{lax,strict}` and `oc_sessionPassphrase` on any page,
//!     whatever the install is called or which sub-path it lives under;
//!   * it NAMES ITS PICTURE in OpenGraph — `og:image` with an `og:image:type`,
//!     which is the share page telling any client where the rendering is.
//!
//! So a share link resolves in two plain GETs and no browser: fetch the page,
//! read what it says about itself, fetch the image it named. Measured against a
//! live Nextcloud 34.0.3 on 2026-09-11: the page is 30 KB of HTML carrying the
//! six `og:` tags in `tests/link_image_share.html`, and the advertised image
//! answers `200 image/jpeg`, 412,697 bytes.
//!
//! The same-origin requirement below is the whole of the trust model: we fetch
//! the image the page pointed at ONLY when it sits on the server that pointed at
//! it, so a page cannot use us to fetch somewhere else.

use url::Url;

/// What a page said about itself.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Advert {
    /// The response carried a cookie whose name is a file-cloud's own.
    pub cloud: bool,
    /// `og:image`, kept only when same-origin with the page that named it.
    pub image: Option<Url>,
    /// `og:image:type`, when given — the page's own word for what it is.
    pub image_type: Option<String>,
}

impl Advert {
    /// Inlineable when the server named itself AND named a picture. Both, because
    /// `og:image` alone is on half the web and says nothing about whether the
    /// thing behind it is a file share we may read.
    pub fn is_inlineable_image(&self) -> bool {
        self.cloud
            && self.image.is_some()
            && self
                .image_type
                .as_deref()
                .is_none_or(|t| t.starts_with("image/"))
    }
}

/// Cookie names a file cloud sets on any page it serves. Names, not paths: an
/// install answers on whatever host and sub-path its owner chose, and none of
/// that reaches here.
fn names_a_file_cloud(cookie_name: &str) -> bool {
    cookie_name.starts_with("nc_sameSiteCookie") || cookie_name == "oc_sessionPassphrase"
}

/// Read a `Set-Cookie` value's name — everything before the first `=`.
pub fn cookie_name(set_cookie: &str) -> &str {
    set_cookie.split('=').next().unwrap_or("").trim()
}

/// What the page says about itself: the product from its cookies, the picture
/// from its OpenGraph tags.
pub fn read_advert<'a>(
    page: &Url,
    set_cookies: impl Iterator<Item = &'a str>,
    html: &str,
) -> Advert {
    let cloud = set_cookies.map(cookie_name).any(names_a_file_cloud);
    let image = meta_property(html, "og:image")
        .and_then(|v| Url::parse(&v).ok())
        // ⚠ Same origin as the page that advertised it, or we are not fetching
        // it. Without this a page could name any address at all and we would
        // dutifully go there — the whole point of fetching server-side is that
        // the server chooses where it goes.
        .filter(|img| same_origin(img, page));
    Advert {
        cloud,
        image,
        image_type: meta_property(html, "og:image:type"),
    }
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// The `content` of `<meta property="NAME" …>`, whichever order the attributes
/// come in and whichever quote the writer used.
///
/// Hand-rolled rather than a parser dependency: this reads two tags out of a
/// head, and a tolerant scan is honest about that. It does NOT understand HTML —
/// it finds `<meta` and reads that tag's attributes to the closing `>`.
fn meta_property(html: &str, property: &str) -> Option<String> {
    let mut rest = html;
    while let Some(at) = rest.find("<meta") {
        let tag_start = at + "<meta".len();
        let tag = &rest[tag_start..];
        let end = tag.find('>').unwrap_or(tag.len());
        let tag = &tag[..end];
        let names_it = attr(tag, "property").as_deref() == Some(property)
            || attr(tag, "name").as_deref() == Some(property);
        if names_it && let Some(c) = attr(tag, "content") {
            return Some(c);
        }
        rest = &rest[tag_start + end.min(tag.len())..];
    }
    None
}

/// One attribute's value out of a tag body.
fn attr(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(at) = rest.find(name) {
        let after = &rest[at + name.len()..];
        let before_ok = at == 0 || rest.as_bytes()[at - 1].is_ascii_whitespace();
        let mut it = after.chars().skip_while(|c| c.is_whitespace());
        if before_ok && it.next() == Some('=') {
            let after_eq = after.trim_start()[1..].trim_start();
            let quote = after_eq.chars().next()?;
            if quote == '"' || quote == '\'' {
                let body = &after_eq[1..];
                return body.find(quote).map(|e| body[..e].to_string());
            }
        }
        rest = &rest[at + name.len()..];
    }
    None
}

/// Every http(s) URL in a line of message text.
///
/// ⚠ Trailing punctuation is NOT part of a link. People write "look at
/// https://host/x." and a naive scan keeps the full stop, which turns a good
/// link into a 404 — and a 404 here is indistinguishable from "not a picture".
pub fn urls_in(text: &str) -> Vec<Url> {
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        let candidate =
            word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '>', '"', '\'']);
        if !(candidate.starts_with("http://") || candidate.starts_with("https://")) {
            continue;
        }
        if let Ok(u) = Url::parse(candidate)
            && !out.contains(&u)
        {
            out.push(u);
        }
    }
    out
}
