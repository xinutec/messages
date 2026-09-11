//! What a pass may claim to have examined.
//!
//! The watermark is the whole correctness story of the backfill: a line behind it
//! is never read again, so moving it over a line whose links were not attempted
//! drops them for ever, silently. `plan` is pure precisely so that rule can be
//! stated here rather than inferred from a run against a database.

use messages::link_fetch::{Line, plan};
use url::Url;

fn line(id: i64, text: &str) -> Line {
    Line {
        id,
        text: text.into(),
    }
}

fn nothing_decided(_: &Url) -> bool {
    false
}

#[test]
fn a_chunk_that_fits_is_examined_whole() {
    let lines = [
        line(10, "see https://a.example/1"),
        line(9, "and https://a.example/2"),
    ];
    let p = plan(&lines, &nothing_decided, 10);
    assert_eq!(p.urls.len(), 2);
    assert_eq!(
        p.examined_through,
        Some(9),
        "the watermark may move past both"
    );
}

#[test]
fn it_stops_before_a_line_it_cannot_finish() {
    // ⚠ The case the whole design turns on. Line 9 carries two links and only one
    // fits; taking one and moving the watermark to 9 would lose the other for
    // ever, because nothing reads behind the watermark again.
    let lines = [
        line(10, "one https://a.example/1"),
        line(9, "two https://a.example/2 https://a.example/3"),
        line(8, "three https://a.example/4"),
    ];
    let p = plan(&lines, &nothing_decided, 2);
    assert_eq!(
        p.urls.iter().map(Url::as_str).collect::<Vec<_>>(),
        ["https://a.example/1"]
    );
    assert_eq!(
        p.examined_through,
        Some(10),
        "stopped at the line it could finish"
    );
}

#[test]
fn a_first_line_that_cannot_be_finished_leaves_the_watermark_alone() {
    let lines = [line(10, "https://a.example/1 https://a.example/2")];
    let p = plan(&lines, &nothing_decided, 1);
    assert!(p.urls.is_empty());
    assert_eq!(
        p.examined_through, None,
        "nothing examined, so nothing to claim"
    );
}

#[test]
fn lines_whose_links_are_all_decided_cost_nothing_and_are_examined() {
    // This is what lets the backfill cross years of GitHub and YouTube links in
    // one run instead of one link at a time.
    let lines = [
        line(10, "https://a.example/old"),
        line(9, "https://a.example/old too"),
        line(8, "https://a.example/new"),
    ];
    let p = plan(&lines, &|u: &Url| u.as_str().ends_with("/old"), 1);
    assert_eq!(
        p.urls.iter().map(Url::as_str).collect::<Vec<_>>(),
        ["https://a.example/new"]
    );
    assert_eq!(p.examined_through, Some(8), "all three examined");
}

#[test]
fn a_line_with_no_links_is_examined() {
    let p = plan(&[line(10, "just talking")], &nothing_decided, 0);
    assert_eq!(p.examined_through, Some(10));
}

#[test]
fn the_same_link_twice_in_a_chunk_is_attempted_once() {
    let lines = [
        line(10, "https://a.example/x"),
        line(9, "https://a.example/x again"),
    ];
    let p = plan(&lines, &nothing_decided, 5);
    assert_eq!(p.urls.len(), 1);
    assert_eq!(p.examined_through, Some(9));
}

#[test]
fn an_empty_chunk_claims_nothing() {
    let p = plan(&[], &nothing_decided, 10);
    assert!(p.urls.is_empty());
    assert_eq!(p.examined_through, None);
}

#[test]
fn the_examined_count_is_the_prefix_that_was_finished() {
    // The log line's number must name what it counts: a chunk is read ahead of
    // what the budget can cover, and reporting its length claimed credit for
    // lines the watermark never moved over.
    let lines = [
        line(10, "https://a.example/1"),
        line(9, "https://a.example/2"),
        line(8, "https://a.example/3"),
    ];
    let p = plan(&lines, &nothing_decided, 2);
    assert_eq!(p.examined_through, Some(9));
    assert_eq!(
        messages::link_fetch::examined_count(&lines, p.examined_through),
        2
    );
    assert_eq!(messages::link_fetch::examined_count(&lines, None), 0);
}
