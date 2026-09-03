import { expect, test, type Page } from "@playwright/test";
// The fleet-shared harness, published as @xinutec/ui-harness (source repo
// ~/Code/ui-harness). Ships compiled JS, so it loads straight from node_modules.
import {
  expectNoTextOverlaps,
  expectNoHorizontalOverflow,
  expectViewportIsPhone,
  expectIconFontLoaded,
} from "@xinutec/ui-harness";

/**
 * L2 phone-width layout harness for messages. Render the two real screens (the
 * conversation-list shell and an open thread) at a Pixel viewport with the
 * backend mocked and BUSY data, and assert the two failure classes that read
 * fine in source and only show on a real phone:
 *   1. no two pieces of rendered text collide, and
 *   2. nothing spills past the right edge.
 * The at-risk spots here: the three-button origin filter row (All / Signal /
 * Google Chat) crowding at 412px, and a message's meta line (sender + time +
 * "edited") and reaction chips overflowing or overlapping the body.
 *
 * There is no service worker in this app, but block it anyway for parity with
 * the fleet's layout specs — SW-controlled fetches would bypass page.route.
 */
test.use({ serviceWorkers: "block" });

const ME = { user_id: "test", display_name: "Test User" };

/** A busy conversation list: all three origins, a group, a deliberately long
 *  name to stress the row title's ellipsis, and a long tail of counts/dates. */
const CONVERSATIONS = [
  { origin: "signal", id: "dm:a", name: "Alice Andersson", kind: "dm", network: null, message_count: 128, last_ts: Date.UTC(2026, 0, 2, 9, 14) },
  { origin: "signal", id: "grp:x", name: "Saturday climbing & bouldering logistics crew", kind: "group", network: null, message_count: 4210, last_ts: Date.UTC(2026, 0, 1, 20, 2) },
  { origin: "gchat", id: "gc1", name: "Bob Bytecode", kind: "dm", network: null, message_count: 37, last_ts: Date.UTC(2025, 11, 30, 16, 40) },
  { origin: "gchat", id: "gc2", name: "Platform on-call", kind: "group", network: null, message_count: 902, last_ts: Date.UTC(2025, 11, 29, 8, 5) },
  // IRC is the only origin with a composer, so it has to be here or the send
  // box is never laid out by this suite — and the composer is the one control
  // that competes for width with anything at phone size.
  { origin: "irc", id: "7", name: "#a-channel-with-a-long-name", kind: "group", network: "xinutec", message_count: 5104, last_ts: Date.UTC(2026, 0, 2, 11, 30) },
  // The same target on two networks — the case the network label exists for, and
  // the widest that subtitle line ever gets at phone width.
  { origin: "irc", id: "8", name: "s_20", kind: "dm", network: "xinutec", message_count: 14446, last_ts: Date.UTC(2026, 0, 2, 10, 15) },
  { origin: "irc", id: "9", name: "s_20", kind: "dm", network: "euirc", message_count: 8071, last_ts: Date.UTC(2026, 0, 1, 22, 40) },
];

/** Real bytes for the one attachment that is `available`. Without them the
 *  <img> 404s and renders as a broken-image glyph, which still satisfies "an img
 *  element exists" — so the test would pass while proving nothing about whether
 *  revealing shows a picture. The live measurement counted LOADED pixels; this
 *  lets the test hold the same standard.
 *
 *  ⚠ SVG rather than a base64 PNG because `Buffer` needs @types/node, which
 *  tsconfig.e2e.json does not carry. Text needs no binary type and still decodes
 *  to something with a naturalWidth. */
const PIXELS =
  '<svg xmlns="http://www.w3.org/2000/svg" width="96" height="64">' +
  '<rect width="96" height="64" fill="#5a6e96"/></svg>';

/** A busy thread: long sender, a long unbroken-ish body, an "edited" tag, an
 *  unavailable attachment, and a row of reaction chips — every element that can
 *  crowd or overflow the bubble. */
const THREAD = {
  messages: [
    { id: "1", ts: Date.UTC(2026, 0, 1, 12, 0), sender: "Alice Andersson", is_outgoing: false,
      body: "Morning! Did the referral letter come through yet? The clinic said they'd post it but it's been almost two weeks now.",
      deleted: false, edited: true, reactions: [{ emoji: "👍", count: 3 }, { emoji: "❤️", count: 2 }, { emoji: "🎉", count: 1 }],
      attachments: [] },
    { id: "2", ts: Date.UTC(2026, 0, 1, 12, 4), sender: "Test User", is_outgoing: true,
      body: "Not yet — chasing them this afternoon.", deleted: false, edited: false, reactions: [],
      attachments: [{ id: "a1", content_type: "application/pdf", file_name: "referral-scan-2026-final-v2.pdf", size: 91234, available: false, is_image: false }] },
    { id: "3", ts: Date.UTC(2026, 0, 1, 12, 9), sender: "Alice Andersson", is_outgoing: false,
      body: "Thankyouuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuu", deleted: false, edited: false, reactions: [], attachments: [] },
    // A deleted message with BOTH halves behind the reveal: words and a stored
    // image. The attachment-only shape is the one that renders no `.body` at
    // all, so it is the one a careless selector misses.
    { id: "4", ts: Date.UTC(2026, 0, 1, 12, 11), sender: "Alice Andersson", is_outgoing: false,
      body: "something said and then taken back", deleted: true, edited: false, reactions: [],
      attachments: [{ id: "a2", content_type: "image/jpeg", file_name: null, size: 4096, available: true, is_image: true }] },
  ],
  has_more: false,
  next_cursor: null,
};

/** Mock every backend call: signed in, the busy list, the busy thread.
 *  Catch-all FIRST — Playwright runs handlers last-registered-first. */
async function mockApi(page: Page): Promise<void> {
  await page.route("**/api/**", (r) =>
    r.request().method() === "GET" ? r.fulfill({ json: [] }) : r.fulfill({ status: 204, body: "" }),
  );
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
  await page.route("**/api/conversations/**/messages**", (r) => r.fulfill({ json: THREAD }));
  // Real bytes for the one attachment that is `available`. Without them the
  // <img> 404s and renders as a broken-image glyph, which still satisfies "an
  // img element exists" — so the test would pass while proving nothing about
  // whether revealing actually shows a picture. The live measurement counted
  // LOADED pixels; this lets the test hold the same standard.
  await page.route("**/api/attachments/**", (r) =>
    r.fulfill({ contentType: "image/svg+xml", body: PIXELS }),
  );
}

// The checker-checker: fail loudly here if the device preset is ever lost and
// the "phone width" suite silently runs at desktop width (defect 2).
test("the suite really runs at phone geometry", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");
  await expectViewportIsPhone(page);
});

test("conversation list — filter row + rows: lays out cleanly @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/");
  await page.getByPlaceholder("Search messages").waitFor();
  await page.getByRole("button", { name: "Google Chat", exact: true }).waitFor(); // widest filter button
  await page.getByText("Alice Andersson").waitFor();
  // Two rows share the title `s_20`; only the network separates them. A template
  // that stopped binding it would still pass every layout assertion below, and
  // the list would go back to showing two rows nobody can tell apart.
  await page.getByText(/IRC xinutec · 14446 msgs/).waitFor();
  await page.getByText(/IRC euirc · 8071 msgs/).waitFor();
  // The search field's prefix icon is where an icon-font fallback shows up as
  // the literal word "search" overlapping the placeholder (guarded here since
  // the shell is the screen with mat-icons).
  await expectIconFontLoaded(page);
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

test("open thread — meta + reactions + attachment: lays out cleanly @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  // Wait for the far messages so the whole thread has laid out before measuring.
  await page.locator(".msg .body").first().waitFor();
  await page.getByText("👍 3").waitFor();
  await page.getByText("referral-scan-2026-final-v2.pdf", { exact: false }).waitFor();
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

test("a deleted message: hidden by default @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  await page.locator(".msg .body").first().waitFor();

  const bubble = page.locator('.msg[data-id="4"]');
  // Absent from the DOM, not merely covered — and the stored image is never
  // fetched. The `deleted` tag in the meta line is what says a message is here
  // at all, so the thread does not silently skip a beat.
  await bubble.getByRole("button", { name: "Show this deleted message" }).waitFor();
  await expect(bubble).not.toContainText("something said and then taken back");
  await expect(bubble.locator("img")).toHaveCount(0);
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

test("a deleted message: revealed on click @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  await page.locator(".msg .body").first().waitFor();

  const bubble = page.locator('.msg[data-id="4"]');
  await bubble.getByRole("button", { name: "Show this deleted message" }).click();
  await expect(bubble).toContainText("something said and then taken back");
  await expect(bubble.locator("img")).toHaveCount(1);
  // Decoded, not merely present — a broken <img> counts as an element too.
  await expect
    .poll(() => bubble.locator("img").first().evaluate((i: HTMLImageElement) => i.naturalWidth))
    .toBeGreaterThan(0);
  // The revealed bubble is the taller one, and an image at phone width is where
  // an overflow would actually show.
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

test("open an IRC thread — the composer: lays out cleanly @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/irc/7");
  await page.locator(".msg .body").first().waitFor();
  // The send button is a mat-icon: an icon-font fallback renders the ligature
  // word "send" instead of the glyph, which is exactly the failure that reads
  // green in vitest because jsdom has no fonts.
  await page.getByRole("button", { name: "Send" }).waitFor();
  await expectIconFontLoaded(page);
  // A long draft is what actually crowds this row — the input, the button, and
  // the phone's width all compete, and a flex item's default min-width is its
  // content, so an unconstrained field pushes the button off the edge.
  // ⚠ `exact` because getByLabel matches on a SUBSTRING: the deleted-message
  // reveal button is named "Show this deleted message" and otherwise resolves
  // here too, failing on strict mode. Two controls a screen reader tells apart
  // perfectly well; it is the locator that has to say which one it means.
  await page.getByLabel("Message", { exact: true }).fill(
    "a fairly long line of the sort somebody actually types on a phone, to see whether the send button survives it",
  );
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

// ⚠ The failure state, which no other case reaches: every test above mocks a
// backend that answers. It is the one most likely to be wrong at phone width,
// because it is the one nobody looks at — a sentence plus a button, in the
// column the conversation list usually fills.
test("search — a failed search says so rather than \"No matches.\" @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.route("**/api/search**", (r) => r.fulfill({ status: 500, body: "boom" }));
  await page.goto("/");
  await page.getByPlaceholder("Search messages").fill("anything");
  await page.getByPlaceholder("Search messages").press("Enter");
  await page.getByText("The search didn't run", { exact: false }).waitFor();
  // The claim it replaces must be absent: rendering both would be worse than
  // rendering only the wrong one.
  await expect(page.locator(".empty")).toHaveCount(0);
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});
