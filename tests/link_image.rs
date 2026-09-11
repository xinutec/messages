//! What the resolver may conclude from a page, and what it must refuse to.
//!
//! The fixture is a REAL Nextcloud 34.0.3 share page (fetched 2026-09-11), with
//! only the host and the share token replaced — this repository is public and a
//! share token is a capability. Its structure, tag order and quoting are the
//! server's own, because a hand-written head would only prove that the reader
//! reads what I expected a head to look like.
//!
//! ⚠ **`.captured-html`, not `.html`, and that is not linter evasion.** The page
//! carries `width=device-width`, so DL-WEB-VIEWPORT-KEYBOARD reads it as one of
//! our app shells relying on the keyboard default. It is not ours — it is
//! evidence of what somebody else's server sends, and the one thing that must
//! never happen to evidence is being edited until a rule is happy with it. The
//! extension says what the file is; the bytes stay as they arrived.

use messages::link_image::{Advert, cookie_name, read_advert, urls_in};
use url::Url;

const SHARE: &str = "https://cloud.example.org/nc/s/SHARETOKEN";
/// The four cookies that live install actually set, in the order it set them —
/// with the VALUES replaced, and the random per-install session cookie renamed.
///
/// ⚠ **The shapes are the evidence; the values were never evidence at all.** The
/// reader only ever looks at a cookie's NAME, so a real passphrase and a real
/// session id sat here proving nothing — they came off a live response along with
/// the rest of the capture, and this repository is public. Caught on review.
/// `oc_sessionPassphrase` and `nc_sameSiteCookie*` are fixed names Nextcloud and
/// ownCloud always set, which is exactly why they can be matched on; the fourth is
/// random per install and is here to show that one arrives and is ignored.
const REAL_COOKIES: [&str; 4] = [
    "oc_sessionPassphrase=REDACTED; path=/nc; secure; HttpOnly; SameSite=Lax",
    "nc_sameSiteCookielax=true; path=/nc; httponly;secure; SameSite=lax",
    "nc_sameSiteCookiestrict=true; path=/nc; httponly;secure; SameSite=strict",
    "ocrandomsession=REDACTED; path=/nc; secure; HttpOnly",
];

fn fixture() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/link_image_share.captured-html"
    ))
    .unwrap()
}

#[test]
fn a_share_page_names_its_product_and_its_picture() {
    let page = Url::parse(SHARE).unwrap();
    let a = read_advert(&page, REAL_COOKIES.into_iter(), &fixture());
    assert!(a.cloud, "the install names itself in the cookies it sets");
    assert_eq!(
        a.image.as_ref().map(Url::as_str),
        Some("https://cloud.example.org/nc/s/SHARETOKEN/preview")
    );
    assert_eq!(a.image_type.as_deref(), Some("image/jpeg"));
    assert!(a.is_inlineable_image());
}

#[test]
fn og_image_alone_is_not_enough() {
    // ⚠ Half the web carries og:image. Without a server that names itself, an
    // inlined "picture" could be any site's social banner — and fetching it
    // would be us browsing the web on someone's behalf, which is not the ask.
    let page = Url::parse(SHARE).unwrap();
    let a = read_advert(&page, ["session=abc; path=/"].into_iter(), &fixture());
    assert!(!a.cloud);
    assert!(!a.is_inlineable_image());
    assert!(
        a.image.is_some(),
        "the tag is still read; it is the verdict that changes"
    );
}

#[test]
fn a_picture_advertised_on_another_server_is_refused() {
    // The trust rule: we fetch what the page named only while the page named
    // something on itself. Otherwise any share page could point us anywhere —
    // at an address on our own side of the VPN, for instance.
    let page = Url::parse(SHARE).unwrap();
    let html = fixture().replace(
        "https://cloud.example.org/nc/s/SHARETOKEN/preview",
        "http://10.100.0.2/internal",
    );
    let a = read_advert(&page, REAL_COOKIES.into_iter(), &html);
    assert!(a.cloud);
    assert_eq!(a.image, None, "off-origin image dropped");
    assert!(!a.is_inlineable_image());
}

#[test]
fn a_page_with_no_picture_is_not_one() {
    let page = Url::parse(SHARE).unwrap();
    let html = fixture().replace("og:image", "og:nothing");
    assert!(!read_advert(&page, REAL_COOKIES.into_iter(), &html).is_inlineable_image());
}

#[test]
fn a_non_image_type_is_refused_even_from_a_cloud() {
    let page = Url::parse(SHARE).unwrap();
    let html = fixture().replace(r#"content="image/jpeg""#, r#"content="application/pdf""#);
    let a = read_advert(&page, REAL_COOKIES.into_iter(), &html);
    assert_eq!(a.image_type.as_deref(), Some("application/pdf"));
    assert!(!a.is_inlineable_image());
}

#[test]
fn cookie_names_are_read_off_the_header() {
    assert_eq!(
        cookie_name("nc_sameSiteCookielax=true; path=/"),
        "nc_sameSiteCookielax"
    );
    assert_eq!(
        cookie_name("oc_sessionPassphrase=x"),
        "oc_sessionPassphrase"
    );
}

#[test]
fn links_are_found_in_what_people_actually_type() {
    // Trailing punctuation is the case that matters: a kept full stop is a 404,
    // and a 404 is indistinguishable from "not a picture".
    let urls = urls_in(
        "look at https://cloud.example.org/nc/s/A. and https://cloud.example.org/nc/s/B, ok",
    );
    assert_eq!(
        urls.iter().map(Url::as_str).collect::<Vec<_>>(),
        [
            "https://cloud.example.org/nc/s/A",
            "https://cloud.example.org/nc/s/B"
        ]
    );
}

#[test]
fn the_same_link_twice_is_one_link() {
    let urls = urls_in("https://cloud.example.org/x https://cloud.example.org/x");
    assert_eq!(urls.len(), 1);
}

#[test]
fn what_is_not_a_link_is_left_alone() {
    assert!(urls_in("ftp://host/f mailto:a@b just words host.example.org/x").is_empty());
}

#[test]
fn an_empty_advert_is_not_inlineable() {
    assert!(!Advert::default().is_inlineable_image());
}

// ---- what a refusal says, and what an attribute carries --------------------

#[test]
fn an_entity_encoded_query_string_is_decoded() {
    // ⚠ THE BUG THIS READER SHIPPED WITH. An og:image with a query string arrives
    // as `?x=1024&amp;y=1024&amp;token=…`, because that is how an attribute
    // spells an ampersand. Fetched raw, the server reads a parameter called
    // `amp;token`, never sees the token, and answers 404 — so the link was
    // recorded "not a picture" about a URL this reader had mangled itself.
    // Measured against the live install: raw → 404 application/json, decoded →
    // 200 image/jpeg.
    let page = Url::parse(SHARE).unwrap();
    let html = fixture().replace(
        r#"content="https://cloud.example.org/nc/s/SHARETOKEN/preview""#,
        r#"content="https://cloud.example.org/nc/s/SHARETOKEN/preview?x=1024&amp;y=1024&amp;token=abc""#,
    );
    let a = read_advert(&page, REAL_COOKIES.into_iter(), &html);
    assert_eq!(
        a.image.as_ref().map(Url::as_str),
        Some("https://cloud.example.org/nc/s/SHARETOKEN/preview?x=1024&y=1024&token=abc"),
        "the ampersands are ampersands, so the token arrives"
    );
}

#[test]
fn a_refusal_says_which_signal_was_missing() {
    // A refusal with no reason becomes permanent and unexplainable: when the
    // verdict was wrong, the control vanished and nothing could say why.
    let page = Url::parse(SHARE).unwrap();
    let not_a_cloud = read_advert(&page, ["session=abc"].into_iter(), &fixture());
    assert_eq!(
        not_a_cloud.refusal(),
        Some("the server does not name itself as a file cloud")
    );

    let no_picture = read_advert(
        &page,
        REAL_COOKIES.into_iter(),
        &fixture().replace("og:image", "og:nothing"),
    );
    assert_eq!(
        no_picture.refusal(),
        Some("no og:image on the page, or it named another server")
    );

    let wrong_type = read_advert(
        &page,
        REAL_COOKIES.into_iter(),
        &fixture().replace(r#"content="image/jpeg""#, r#"content="application/pdf""#),
    );
    assert_eq!(
        wrong_type.refusal(),
        Some("og:image:type is not an image type")
    );

    assert_eq!(
        read_advert(&page, REAL_COOKIES.into_iter(), &fixture()).refusal(),
        None
    );
}

#[test]
fn a_verdict_from_an_older_reader_may_be_asked_again() {
    use messages::link_image::{LinkState, READER_VERSION, askable};

    // A picture we hold needs no asking; everything else may be asked again if
    // the reader that decided it has since been fixed.
    assert!(!askable(LinkState::Ok, Some(READER_VERSION)));
    assert!(askable(LinkState::Offered, None));
    assert!(
        askable(LinkState::Failed, Some(READER_VERSION)),
        "a server that was down is not a server that is"
    );
    assert!(
        !askable(LinkState::NotImage, Some(READER_VERSION)),
        "this reader looked and said no"
    );
    assert!(
        askable(LinkState::NotImage, Some(READER_VERSION - 1)),
        "an older reader said no, and readers get fixed"
    );
    assert!(
        askable(LinkState::NotImage, None),
        "decided before we recorded which reader decided"
    );
}
