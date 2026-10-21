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

/// What is known about a link, spelled as the table's enum. Only what we hold
/// is written down: "not a picture" and "could not reach it" are told to the
/// reader once, since both the page and what counts as a picture change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// Registered from a message; may be asked for.
    Offered,
    /// The bytes are on the volume.
    Ok,
}

impl LinkState {
    /// Parse once, at the boundary where the row is read.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "offered" => Some(Self::Offered),
            "ok" => Some(Self::Ok),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Offered => "offered",
            Self::Ok => "ok",
        }
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

    /// Why this page is not an inlineable picture, logged so a wrong verdict can
    /// be explained.
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
    // `&amp;` last, or `&amp;lt;`, the text `&lt;`, would be decoded twice.
    raw.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
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

/// The links in a message's text, by the rule the page links them with
/// (frontend `formatting.ts` `addresses`, held to one table:
/// `testdata/links-in-text.json`): an address starts with `http://`, `https://`
/// or `www.` at a word's start, in any case, and runs to a space, `<`, `>` or
/// `"`; closing punctuation, and a `)` the address did not open, belong to the
/// sentence. `www.` alone is fetched over https, as the page links it.
pub fn urls_in(text: &str) -> Vec<Url> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let starts = ["http://", "https://", "www."].iter().any(|p| {
            rest.get(..p.len())
                .is_some_and(|h| h.eq_ignore_ascii_case(p))
        });
        let at_word_start = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if !(starts && at_word_start) {
            i += rest.chars().next().map_or(1, char::len_utf8);
            continue;
        }
        let len = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
            .unwrap_or(rest.len());
        let address = trimmed(&rest[..len]);
        i += len.max(1);
        let full = if address
            .get(..4)
            .is_some_and(|h| h.eq_ignore_ascii_case("www."))
        {
            format!("https://{address}")
        } else {
            address.to_owned()
        };
        if let Ok(u) = Url::parse(&full)
            && !out.contains(&u)
        {
            out.push(u);
        }
    }
    out
}

/// An address without what the sentence put after it: closing punctuation, and
/// a `)` the address itself did not open.
fn trimmed(address: &str) -> &str {
    let mut s = address;
    loop {
        match s.chars().next_back() {
            Some('.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"') => s = &s[..s.len() - 1],
            Some(')') if s.matches(')').count() > s.matches('(').count() => s = &s[..s.len() - 1],
            _ => return s,
        }
    }
}
