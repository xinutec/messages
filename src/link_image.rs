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

/// Which reader made a decision.
///
/// ⚠ **A VERDICT IS ONLY AS GOOD AS THE READER THAT MADE IT, and storing one
/// without saying which reader made it makes a bug permanent.** On 2026-09-11
/// this reader fetched `…?x=1024&amp;y=1024&amp;token=…` literally, so the
/// server saw parameters named `amp;token`, answered 404, and the link was
/// recorded "not a picture" for ever — the control disappeared from the
/// conversation and no amount of fixing the bug brought it back.
///
/// Bump this whenever what counts as a picture changes. Decisions made by an
/// older reader are offered again rather than believed.
pub const READER_VERSION: i32 = 2;

/// What is known about a link. A closed set, and the table's enum is its
/// spelling: `offered` before anyone asks, and one of the three verdicts after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// Registered from a message, with nothing fetched.
    Offered,
    /// The bytes are on the volume.
    Ok,
    /// Reached it; not a picture we may inline.
    NotImage,
    /// Could not reach it, or it broke the limits.
    Failed,
}

impl LinkState {
    /// Parse once, at the boundary where the row is read.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "offered" => Some(Self::Offered),
            "ok" => Some(Self::Ok),
            "not_image" => Some(Self::NotImage),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Offered => "offered",
            Self::Ok => "ok",
            Self::NotImage => "not_image",
            Self::Failed => "failed",
        }
    }
}

/// Whether a link may still be asked for.
///
/// ⚠ **ONE PREDICATE, BECAUSE TWO WOULD DRIFT.** Serving a page uses it to
/// decide whether to draw a control, and the request endpoint uses it to decide
/// whether a hash resolves to an address. If they disagree by so much as a state,
/// the reader gets a button that answers 404 — which is exactly what a first cut
/// of this did, offering rows the request path refused.
///
///   * `offered` — registered, never asked about
///   * `failed`  — the far side was unreachable, which is a fact about that
///     afternoon rather than about the link
///   * anything decided by an OLDER reader — see `READER_VERSION`
///   * never `ok`: we hold the picture, so there is nothing to ask
pub fn askable(state: LinkState, decided_by: Option<i32>) -> bool {
    match state {
        LinkState::Ok => false,
        LinkState::Offered | LinkState::Failed => true,
        LinkState::NotImage => decided_by.is_none_or(|v| v < READER_VERSION),
    }
}

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
        self.refusal().is_none()
    }

    /// Why this page is not an inlineable picture, in the words of what was
    /// missing.
    ///
    /// ⚠ **A REFUSAL WITH NO REASON BECOMES PERMANENT AND UNEXPLAINABLE.** That
    /// sentence was already in this codebase, about the failure path, and was not
    /// applied to the refusal that actually fires: a link decided "not a picture"
    /// was stored with a NULL note, so when the verdict was wrong — a URL this
    /// reader had mangled itself, 2026-09-11 — the control vanished from the
    /// conversation and nothing anywhere could say why.
    pub fn refusal(&self) -> Option<&'static str> {
        if !self.cloud {
            return Some("the server does not name itself as a file cloud");
        }
        if self.image.is_none() {
            return Some("no og:image on the page, or it named another server");
        }
        if !self
            .image_type
            .as_deref()
            .is_none_or(|t| t.starts_with("image/"))
        {
            return Some("og:image:type is not an image type");
        }
        None
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

/// The five entities an HTML attribute may carry, decoded.
///
/// ⚠ **`&amp;` IS THE WHOLE OF WHY THIS EXISTS, and it cost a wrong verdict.**
/// An `og:image` with a query string arrives as
/// `…/preview?x=1024&amp;y=1024&amp;token=…`, because that is how an attribute
/// spells an ampersand. Fetched raw, the server reads parameters called `amp;y`
/// and `amp;token`, the token never arrives, and it answers 404 — so the fetcher
/// concluded "not a picture" about a URL it had mangled itself. Measured
/// 2026-09-11 against a live Nextcloud: raw → 404 application/json, decoded →
/// 200 image/jpeg.
///
/// Hand-rolled like the rest of this reader, and the list is short because an
/// attribute value cannot contain a raw `<` or `&`: these five are what a
/// conforming writer emits, and anything else is left alone rather than guessed
/// at.
fn decode_entities(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_owned();
    }
    raw.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
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
                return body.find(quote).map(|e| decode_entities(&body[..e]));
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
