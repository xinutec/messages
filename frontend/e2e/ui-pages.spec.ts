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
/** ⚠ THE ANDROID KEYBOARD, which is where a chat composer usually goes wrong.
 *
 *  `index.html` asks for `interactive-widget=resizes-content`, so the soft
 *  keyboard shrinks the LAYOUT viewport rather than sliding over it — which is
 *  precisely what `setViewportSize` does, so this is the real geometry and not
 *  an approximation of it.
 *
 *  Both mechanisms it guards were ablated and both fail it: dropping the
 *  composer's `position: sticky`, and `height: 100vh` on the thread in place of
 *  `100%`. The vh one is not "vh does not shrink" — it does — it is that vh
 *  measures the whole viewport and ignores the shell above it, so the composer
 *  lands below the fold.
 *
 *  ⚠ **What this canNOT see is the meta token itself.** Remove
 *  `interactive-widget=resizes-content` and a real phone stops shrinking the
 *  layout viewport at all, while this test goes on shrinking it directly and
 *  stays green. That half is pinned by the test below, which is the whole
 *  reason there are two. */
test("the composer stays above the Android keyboard @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/irc/7");
  const input = page.getByLabel("Message", { exact: true });
  await input.waitFor();
  await input.fill("half a sentence, still being typed");

  const full = page.viewportSize()!;
  const KEYBOARD = 350; // a Pixel's, near enough
  await page.setViewportSize({ width: full.width, height: full.height - KEYBOARD });

  // Wholly on screen: not clipped at the bottom, not pushed off the top.
  const box = (await input.boundingBox())!;
  expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.y + box.height).toBeLessThanOrEqual(full.height - KEYBOARD);
  // And the button that sends it, which is the half that gets squeezed out.
  const send = (await page.getByRole("button", { name: "Send" }).boundingBox())!;
  expect(send.y + send.height).toBeLessThanOrEqual(full.height - KEYBOARD);
  expect(send.x + send.width).toBeLessThanOrEqual(full.width);
  // What was typed is still there, and still the value being edited.
  await expect(input).toHaveValue("half a sentence, still being typed");

  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

/** ⚠ ONE TOKEN, AND NOTHING ELSE KNEW ABOUT IT. `interactive-widget=
 *  resizes-content` is what makes the Android soft keyboard shrink the layout
 *  viewport instead of sliding over the page; without it the composer sits
 *  behind the keys and every geometry test above still passes, because they
 *  resize the viewport themselves.
 *
 *  It lives in a generated-looking file that an `ng update` may rewrite, it had
 *  no comment, and losing it is invisible everywhere except on a real phone with
 *  a real keyboard. Asserted against the SERVED page rather than the source, so
 *  a build step that drops it is caught too. */
test("the served page asks the keyboard to shrink the layout viewport", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");
  const content = await page.locator('meta[name="viewport"]').getAttribute("content");
  expect(content).toContain("interactive-widget=resizes-content");
});

/** ⚠ The IME half of "does it work while you are typing", driven by a REAL
 *  composition rather than a hand-built KeyboardEvent. `Input.imeSetComposition`
 *  puts Chromium into the same state Gboard does mid-word, so the Enter that
 *  follows carries `isComposing` for real.
 *
 *  The unit test asserts the guard; this asserts that the browser actually
 *  reports the state the guard reads, which is the assumption the unit test has
 *  to make and cannot check. */
test("Enter while the IME is composing does not send @ phone width", async ({ page, context }) => {
  await mockApi(page);
  let sends = 0;
  await page.route("**/api/conversations/*/*/send", async (r) => {
    sends += 1;
    await r.fulfill({ json: { sent: true, error: null, archived: true } });
  });
  await page.goto("/conversation/irc/7");
  const input = page.getByLabel("Message", { exact: true });
  await input.click();

  const cdp = await context.newCDPSession(page);
  await cdp.send("Input.imeSetComposition", {
    text: "hello wor",
    selectionStart: 9,
    selectionEnd: 9,
  });
  await page.keyboard.press("Enter");
  expect(sends).toBe(0);

  // And a plain Enter, with nothing composing, still sends — otherwise this
  // would pass just as well against a composer that never sends at all.
  await cdp.send("Input.imeSetComposition", { text: "", selectionStart: 0, selectionEnd: 0 });
  await input.fill("a finished sentence");
  await page.keyboard.press("Enter");
  await expect.poll(() => sends).toBe(1);
});

/** Search results, as the list renders them. Two ordinary hits and a retracted
 *  one, whose `snippet` still carries the withdrawn words — the server sends
 *  them and the reader is what hides them, so a fixture with the text stripped
 *  out would be testing a backend that does not exist. */
const SEARCH = [
  { origin: "signal", conversation_id: "dm:a", conversation_name: "Alice Andersson", ts: Date.UTC(2026, 0, 2, 9, 14),
    sender: "Alice Andersson", snippet: "the referral letter finally turned up this morning, second post", deleted: false },
  { origin: "signal", conversation_id: "dm:a", conversation_name: "Alice Andersson", ts: Date.UTC(2026, 0, 1, 18, 3),
    sender: "Alice Andersson", snippet: "posted the letter on Tuesday", deleted: true },
  { origin: "irc", conversation_id: "7", conversation_name: "#a-channel-with-a-long-name", ts: Date.UTC(2025, 11, 30, 16, 40),
    sender: "s_20", snippet: "no letter here, wrong channel", deleted: false },
  // ⚠ The same target on two networks — two rows the list can tell apart and
  // this one could not. Both are in CONVERSATIONS, ids 8 and 9.
  { origin: "irc", conversation_id: "8", conversation_name: "s_20", ts: Date.UTC(2025, 11, 29, 12, 0),
    sender: "s_20", snippet: "letter sent, check the pigeon", deleted: false },
  { origin: "irc", conversation_id: "9", conversation_name: "s_20", ts: Date.UTC(2025, 11, 28, 12, 0),
    sender: "s_20", snippet: "letter never arrived", deleted: false },
];

/** ⚠ **A RETRACTION IS FOUND WITHOUT BEING SPELLED OUT.** Search stopped
 *  filtering `deleted` rows in SQL on 2026-09-04, so the withdrawn text now
 *  reaches the browser and only the template keeps it off the list.
 *
 *  jsdom pins the absence already (`app.spec.ts`). What needs a real render is
 *  the other half: `(deleted)` has to read as a retraction sitting among
 *  ordinary hits rather than as something somebody said, and the extra inline
 *  span must not crowd a list row that already truncates at phone width. */
test("search — a retracted hit is listed without its text @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.route("**/api/search**", (r) => r.fulfill({ json: SEARCH }));
  await page.goto("/");
  await page.getByPlaceholder("Search messages").fill("letter");
  await page.getByPlaceholder("Search messages").press("Enter");

  const list = page.locator("mat-action-list");
  await list.getByText("(deleted)").waitFor();
  // The words themselves, nowhere on the screen.
  await expect(page.locator("body")).not.toContainText("posted the letter on Tuesday");
  // And the hit is still a hit — five rows, the retracted one among them.
  await expect(list.getByRole("button")).toHaveCount(5);
  await expect(list).toContainText("the referral letter finally turned up");

  // ⚠ The two `s_20` rows are told apart, by the same network label the
  // conversation list carries. Without it these are one row twice. Located by
  // their snippets: title AND sender are `s_20` on both, which is the point.
  const rows = list.getByRole("button");
  await expect(rows.filter({ hasText: "letter sent, check the pigeon" })).toContainText("IRC xinutec");
  await expect(rows.filter({ hasText: "letter never arrived" })).toContainText("IRC euirc");

  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

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
