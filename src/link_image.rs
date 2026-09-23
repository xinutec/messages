//! Deciding, from a fetched page alone, whether a link in a message is a
//! picture we can serve ourselves.
//!
//! Nothing here knows a host, path or product URL. An inlineable server says so
//! itself: it names its product in the cookies it sets (Nextcloud and ownCloud
//! set `nc_sameSiteCookie{lax,strict}` and `oc_sessionPassphrase`), and names
//! its picture in OpenGraph (`og:image`, `og:image:type`). So a share link
//! resolves in two GETs: the page, then the image it names, and only if that
//! image is on the same origin as the page.

use url::Url;

/// Which reader made a decision.
///
/// Bump this whenever what counts as a picture changes: decisions made by an
/// older reader are offered again rather than believed.
pub const READER_VERSION: i32 = 2;

/// What is known about a link, spelled as the table's enum: `offered` before
/// anyone asks, then one of three verdicts.
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
/// Serving a page and the request endpoint both use this, so a control is never
/// drawn for a link that cannot be requested.
///
///   * `offered` — registered, never asked about
///   * `failed`  — unreachable at the time, not a verdict on the link
///   * anything decided by an older reader; see `READER_VERSION`
///   * never `ok`: the picture is held
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
    /// `og:image:type`, when given.
    pub image_type: Option<String>,
}

impl Advert {
    /// Inlineable when the server named itself and named a picture; `og:image`
    /// alone is on half the web.
    pub fn is_inlineable_image(&self) -> bool {
        self.refusal().is_none()
    }

    /// Why this page is not an inlineable picture, stored with the verdict so a
    /// wrong one can be explained.
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

/// Cookie names a file cloud sets on any page, whatever its host and path.
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
        // Same origin as the page that named it, or not fetched.
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
/// A tolerant scan, not an HTML parser: it finds `<meta` and reads that tag's
/// attributes.
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
/// An `og:image` URL arrives as `…?x=1024&amp;y=1024&amp;token=…`; fetched
/// undecoded, the server never sees the token. A conforming writer emits only
/// these five; anything else is left alone.
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
/// Trailing punctuation is not part of a link.
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
