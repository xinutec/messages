//! What the resolver may conclude from a page, and what it must refuse to.
//!
//! The fixture is a real Nextcloud share page with the host and share token
//! replaced; structure, tag order and quoting are the server's own. Named
//! `.captured-html` because it is somebody else's page, not one of our app
//! shells for DL-WEB-VIEWPORT-KEYBOARD to judge.

use messages::link_image::{Advert, cookie_name, read_advert, urls_in};
use url::Url;

const SHARE: &str = "https://cloud.example.org/nc/s/SHARETOKEN";
/// The cookies that install set, in order, with values replaced: the reader
/// looks only at names. `oc_sessionPassphrase` and `nc_sameSiteCookie*` are
/// fixed names; the fourth is random per install and must be ignored.
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
    // og:image alone is on half the web; the server must name itself too.
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
    // The page may only name an image on its own origin, or it could point us
    // anywhere, our side of the VPN included.
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
    // A kept full stop would 404.
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
    // An attribute spells `&` as `&amp;`; undecoded, the token never arrives.
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
    // A refusal carries its reason.
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

    // A held picture needs no asking; anything else may be asked again once
    // the reader that decided it has been fixed.
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
