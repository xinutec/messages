import { expect, test, type Page } from "@playwright/test";
// The fleet-shared harness, @xinutec/ui-harness (~/Code/ui-harness).
import {
  expectNoTextOverlaps,
  expectNoHorizontalOverflow,
  expectViewportIsPhone,
  expectIconFontLoaded,
} from "@xinutec/ui-harness";

/**
 * Phone-width layout checks: the conversation list and an open thread at a
 * Pixel viewport, with the backend mocked and busy data. Asserts that no two
 * pieces of text collide and nothing spills past the right edge.
 */

const ME = { user_id: "test", display_name: "Test User" };

/** A busy conversation list: every origin, a group, and a long name. */
const CONVERSATIONS = [
  { origin: "signal", id: "dm:a", name: "Alice Andersson", kind: "dm", network: null, message_count: 128, last_ts: Date.UTC(2026, 0, 2, 9, 14) },
  { origin: "signal", id: "grp:x", name: "Saturday climbing & bouldering logistics crew", kind: "group", network: null, message_count: 4210, last_ts: Date.UTC(2026, 0, 1, 20, 2) },
  { origin: "gchat", id: "gc1", name: "Bob Bytecode", kind: "dm", network: null, message_count: 37, last_ts: Date.UTC(2025, 11, 30, 16, 40) },
  { origin: "gchat", id: "gc2", name: "Platform on-call", kind: "group", network: null, message_count: 902, last_ts: Date.UTC(2025, 11, 29, 8, 5) },
  // IRC, the only origin with a composer.
  { origin: "irc", id: "7", name: "#a-channel-with-a-long-name", kind: "group", network: "xinutec", message_count: 5104, last_ts: Date.UTC(2026, 0, 2, 11, 30) },
  // One target on two networks, the widest subtitle.
  { origin: "irc", id: "8", name: "s_20", kind: "dm", network: "xinutec", message_count: 14446, last_ts: Date.UTC(2026, 0, 2, 10, 15) },
  { origin: "irc", id: "9", name: "s_20", kind: "dm", network: "euirc", message_count: 8071, last_ts: Date.UTC(2026, 0, 1, 22, 40) },
];

/** Real bytes for the available attachment, so a revealed image actually
 *  decodes rather than rendering a broken glyph. SVG, because `Buffer` needs
 *  @types/node, which tsconfig.e2e.json lacks. */
const PIXELS =
  '<svg xmlns="http://www.w3.org/2000/svg" width="96" height="64">' +
  '<rect width="96" height="64" fill="#5a6e96"/></svg>';

/** A busy thread: every element that can crowd or overflow a bubble. */
const THREAD = {
  messages: [
    { id: "1", ts: Date.UTC(2026, 0, 1, 12, 0), sender: "Alice Andersson", is_outgoing: false,
      body: "Morning! Did the referral letter come through yet? The clinic said they'd post it but it's been almost two weeks now.",
      deleted: false, edited: true, reply_to: null, delivery: null, entities: [], album: null, reactions: [{ emoji: "👍", count: 3, who: ["Bob Bytecode", "Dana", "Test User"] }, { emoji: "❤️", count: 2, who: [] }, { emoji: "🎉", count: 1, who: ["Dana"] }],
      attachments: [], link_images: [], link_offers: [], edits: [] },
    // A full-length reply excerpt on an outgoing bubble, the narrowest.
    { id: "2", ts: Date.UTC(2026, 0, 1, 12, 4), sender: "Test User", is_outgoing: true,
      body: "Not yet — chasing them this afternoon.", deleted: false, edited: false, reactions: [],
      // Two readers, on the narrowest bubble; rendered through a DM route and a
      // group route below.
      delivery: { state: "read", read_by: [{ who: "Alice Andersson", at: Date.UTC(2026, 0, 1, 12, 6) }, { who: "Bob Bytecode", at: Date.UTC(2026, 0, 1, 12, 8) }] },
      reply_to: { id: "1", cursor: "1_1", ts: Date.UTC(2026, 0, 1, 12, 0), sender: "Alice Andersson",
        excerpt: "Morning! Did the referral letter come through yet? The clinic said they'd post it but it's been almost two weeks n…",
        deleted: false },
      attachments: [{ id: "a1", content_type: "application/pdf", file_name: "referral-scan-2026-final-v2.pdf", size: 91234, available: false, is_image: false }], link_images: [], link_offers: [], edits: [] },
    // An unresolved quote, the longer wording.
    { id: "3", ts: Date.UTC(2026, 0, 1, 12, 9), sender: "Alice Andersson", is_outgoing: false,
      body: "Thankyouuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuu", deleted: false, edited: false, reactions: [], delivery: null,
      reply_to: { id: null, cursor: null, ts: Date.UTC(2025, 5, 3, 8, 30), sender: null, excerpt: null, deleted: false },
      attachments: [], link_images: [], link_offers: [], edits: [] },
    // Deleted, with words and a stored image behind the reveal.
    { id: "4", ts: Date.UTC(2026, 0, 1, 12, 11), sender: "Alice Andersson", is_outgoing: false,
      body: "something said and then taken back", deleted: true, edited: false, reactions: [], reply_to: null, delivery: null, entities: [], album: null,
      attachments: [{ id: "a2", content_type: "image/jpeg", file_name: null, size: 4096, available: true, is_image: true }], link_images: [], link_offers: [], edits: [] },
    // A Telegram service event, rendered as an action.
    { id: "5", ts: Date.UTC(2026, 0, 1, 12, 20), sender: "Alice Andersson", is_outgoing: false,
      kind: "action", body: "made a 55-minute video call", deleted: false, edited: false,
      reactions: [], reply_to: null, delivery: null, entities: [], album: null,
      attachments: [], link_images: [], link_offers: [], edits: [] },
    // Named reactors: the names are in a title and must not widen the chip.
    { id: "6", ts: Date.UTC(2026, 0, 1, 12, 24), sender: "Alice Andersson", is_outgoing: false,
      body: "Six of us are in.", deleted: false, edited: false, reply_to: null, delivery: null, entities: [], album: null,
      reactions: [{ emoji: "👍", count: 6, who: ["Alice Andersson", "Bob Bytecode", "Test User", "Dana", "Erin Example"] },
                  { emoji: "🎉", count: 1, who: ["Dana"] }],
      attachments: [], link_images: [], link_offers: [], edits: [] },
    // An outgoing album of three: a captioned member, then two pictures.
    { id: "7", ts: Date.UTC(2026, 0, 1, 12, 30), sender: "Test User", is_outgoing: true,
      body: "From the climbing wall on Saturday, the three of us at the top of the orange route", deleted: false, edited: false, reactions: [], reply_to: null, entities: [], album: "9007199254740993",
      delivery: { state: "delivered", read_by: [] },
      attachments: [{ id: "p7", content_type: "image/jpeg", file_name: null, size: 4096, available: true, is_image: true }], link_images: [], link_offers: [], edits: [] },
    { id: "8", ts: Date.UTC(2026, 0, 1, 12, 30), sender: "Test User", is_outgoing: true,
      body: null, deleted: false, edited: false, reactions: [], reply_to: null, entities: [], album: "9007199254740993",
      delivery: { state: "delivered", read_by: [] },
      attachments: [{ id: "p8", content_type: "image/jpeg", file_name: null, size: 4096, available: true, is_image: true }], link_images: [], link_offers: [], edits: [] },
    { id: "9", ts: Date.UTC(2026, 0, 1, 12, 30), sender: "Test User", is_outgoing: true,
      body: null, deleted: false, edited: false, reactions: [], reply_to: null, entities: [], album: "9007199254740993",
      delivery: { state: "delivered", read_by: [] },
      attachments: [{ id: "p9", content_type: "image/jpeg", file_name: null, size: 4096, available: true, is_image: true }], link_images: [], link_offers: [], edits: [] },
  ],
  has_more: false,
  next_cursor: null,
};

/** A thread taller than the pane, so there is a scroll position the keyboard
 *  can disturb. */
const LONG_THREAD = {
  messages: Array.from({ length: 60 }, (_, i) => ({
    id: String(i + 1),
    ts: Date.UTC(2026, 0, 2, 10, 0) + i * 60_000,
    sender: i % 2 ? "Test User" : "Alice Andersson",
    is_outgoing: i % 2 === 1,
    body: `line number ${i + 1} of the conversation`,
    deleted: false,
    edited: false,
    reply_to: null,
    // Every delivery rung is laid out; `delivered` is the longest word.
    delivery: i % 2 === 1 ? { state: ["sent", "delivered", "read"][(i >> 1) % 3], read_by: [] } : null,
    reactions: [],
    attachments: [], link_images: [], link_offers: [], edits: [],
  })),
  has_more: false,
  next_cursor: null,
  prev_cursor: null,
};

/** Mock every backend call. The catch-all goes first: Playwright runs handlers
 *  last-registered-first. */
async function mockApi(page: Page): Promise<void> {
  await page.route("**/api/**", (r) =>
    r.request().method() === "GET" ? r.fulfill({ json: [] }) : r.fulfill({ status: 204, body: "" }),
  );
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
  await page.route("**/api/conversations/**/messages**", (r) => r.fulfill({ json: THREAD }));
  // See PIXELS.
  await page.route("**/api/attachments/**", (r) =>
    r.fulfill({ contentType: "image/svg+xml", body: PIXELS }),
  );
}

// Fails if the device preset is lost and the suite runs at desktop width.
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
  // Two rows titled `s_20`; only the network separates them.
  await page.getByText(/IRC xinutec · 14446 msgs/).waitFor();
  await page.getByText(/IRC euirc · 8071 msgs/).waitFor();
  // An icon-font fallback would show the word "search" over the placeholder.
  await expectIconFontLoaded(page);
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

test("open thread — meta + reactions + attachment: lays out cleanly @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  // Wait for the far messages, so the thread has laid out.
  await page.locator(".msg .body").first().waitFor();
  await page.getByText("👍 3").waitFor();
  await page.getByText("referral-scan-2026-final-v2.pdf", { exact: false }).waitFor();
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

/// An album renders as one set: its members in one grid, under one meta line.
test("an album is one set under one meta line @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  const album = page.locator(".run.album");
  await expect(album).toHaveCount(1);
  await expect(album.locator(".msg")).toHaveCount(3);
  await expect(album.locator(".meta:visible")).toHaveCount(1);
  await album.scrollIntoViewIfNeeded();
  await expect
    .poll(() => album.locator("img").evaluateAll((els) =>
      els.filter((e) => e instanceof HTMLImageElement && e.complete && e.naturalWidth > 0).length))
    .toBe(3);
  await page.screenshot({ path: testInfo.outputPath("album.png") });
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

/// Who reacted lives in a `title`, which cannot change the chip's size, and
/// `count` stays the number.
test("who reacted is a hover, and the chip still says the count @ phone width", async ({ page }) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  const chip = page.getByText("👍 6");
  await chip.waitFor();
  await expect(chip).toHaveAttribute(
    "title",
    "Alice Andersson, Bob Bytecode, Test User, Dana, Erin Example and 1 more",
  );
  // A reaction with no names gets no hover.
  await expect(page.getByText("❤️ 2")).toHaveAttribute("title", "");
});

/// Two receipts are everybody in a DM, and two people in a group.
test("a group counts its readers; a DM says read @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  const tag = () => page.locator('.msg[data-id="2"] .tag.delivery');

  await page.goto("/conversation/signal/dm:a");
  await tag().waitFor();
  await expect(tag()).toHaveText("read");
  // Names and times live in the hover, like reaction chips.
  await expect(tag()).toHaveAttribute("title", /Alice Andersson.*Bob Bytecode/);

  await page.goto("/conversation/signal/grp:x");
  await tag().waitFor();
  await expect(tag()).toHaveText("read by 2");
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

/// The shell's search box is hidden at phone width with a thread open, so the
/// thread carries its own.
test("searching a conversation is reachable with the thread open @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.route("**/api/search**", (r) =>
    r.fulfill({
      json: [
        { origin: "signal", conversation_id: "dm:a", conversation_name: "Alice Andersson",
          ts: Date.UTC(2025, 4, 3, 14, 2), sender: "Alice Andersson",
          snippet: "a phrase that appears nowhere in the thread", deleted: false, cursor: "c1" },
        { origin: "signal", conversation_id: "dm:a", conversation_name: "Alice Andersson",
          ts: Date.UTC(2024, 8, 9, 9, 30), sender: "Test User",
          snippet: "something withdrawn", deleted: true, cursor: "c2" },
      ],
    }),
  );
  await page.goto("/conversation/signal/dm:a");
  await page.locator(".msg .body").first().waitFor();

  // The shell's box is gone, so this found the thread's.
  await expect(page.getByPlaceholder("Search messages")).toBeHidden();

  await page.getByRole("button", { name: "Search this conversation" }).click();
  const box = page.getByPlaceholder("Search this conversation");
  await box.fill("referral");
  await box.press("Enter");
  // Scoped to the panel: the thread's bodies contain these words too.
  const panel = page.locator(".thread-search");
  await panel.getByText("a phrase that appears nowhere", { exact: false }).waitFor();

  // A retracted hit shows who and when, not the words.
  await expect(panel.getByText("Test User: (deleted)")).toBeVisible();
  await expect(panel.getByText("something withdrawn")).toHaveCount(0);

  // Scoped to the panel: a day header pins behind it, which a geometric scan
  // cannot tell from a collision.
  await expectNoTextOverlaps(page, testInfo, ".thread-search");
  await expectNoHorizontalOverflow(page, testInfo);
});

/// "No matches" is a claim, so a failed request must not make it.
test("a failed conversation search says so rather than \"no matches\" @ phone width", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/search**", (r) => r.fulfill({ status: 500, body: "" }));
  await page.goto("/conversation/signal/dm:a");
  await page.locator(".msg .body").first().waitFor();
  await page.getByRole("button", { name: "Search this conversation" }).click();
  const box = page.getByPlaceholder("Search this conversation");
  await box.fill("anything");
  await box.press("Enter");
  await expect(page.getByText("Couldn't search.")).toBeVisible();
  await expect(page.getByText("No matches in this conversation.")).toHaveCount(0);
});

/// The date goes to the server as local midnight in milliseconds; the server
/// converts it to the origin's unit.
test("picking a date asks the server for that day @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  const asked: string[] = [];
  await page.route("**/messages**", (r) => {
    asked.push(r.request().url());
    return r.fulfill({ json: THREAD });
  });
  await page.goto("/conversation/signal/dm:a");
  await page.locator(".msg .body").first().waitFor();

  await page.locator('.thread-head input[type="date"]').fill("2025-03-04");
  await page.waitForFunction(() => location.search.includes("on="));

  const expected = String(new Date(2025, 2, 4).getTime());
  expect(page.url()).toContain(`on=${expected}`);
  // Both halves of the landing carry it.
  const withOn = asked.filter((u) => u.includes(`on=${expected}`));
  expect(withOn.some((u) => u.includes("dir=older"))).toBe(true);
  expect(withOn.some((u) => u.includes("dir=at"))).toBe(true);

  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

/// Formatting renders from the body: the visible text is exactly what was sent.
test("telegram formatting renders from the body, never from the entity @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  const body = "👋 bold here, a link https://example.com/x and a secret plus unknownfmt";
  await page.route("**/messages**", (r) =>
    r.fulfill({
      json: {
        messages: [{
          id: "1", ts: Date.UTC(2026, 0, 1, 12, 0), sender: "Alice Andersson", is_outgoing: false,
          kind: "message", body, deleted: false, edited: false, reactions: [], attachments: [],
          link_images: [], link_offers: [], edits: [], reply_to: null, delivery: null,
          entities: [
            // UTF-16 units: the leading 👋 is two, so "bold" starts at 3.
            { kind: "bold", offset: 3, length: 4, url: null },
            { kind: "url", offset: 21, length: 21, url: null },
            { kind: "spoiler", offset: 49, length: 6, url: null },
            // An unknown kind renders as plain text.
            { kind: "someFutureKind", offset: 61, length: 10, url: null },
          ],
        }],
        has_more: false, next_cursor: null, prev_cursor: null,
      },
    }),
  );
  await page.goto("/conversation/telegram/4242");
  const bodyEl = page.locator(".msg .body").first();
  await bodyEl.waitFor();

  // The whole message, unchanged.
  expect((await bodyEl.innerText()).trim()).toBe(body);

  await expect(bodyEl.locator(".fmt-bold")).toHaveText("bold");
  await expect(bodyEl.locator(".fmt-spoiler")).toHaveText("secret");
  await expect(bodyEl.locator("a")).toHaveAttribute("href", "https://example.com/x");
  // A spoiler is covered, not invisible.
  await expect(bodyEl.locator(".fmt-spoiler")).not.toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

test("a deleted message: hidden by default @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  await page.locator(".msg .body").first().waitFor();

  const bubble = page.locator('.msg[data-id="4"]');
  // Absent, not covered, and the image never fetched; the `deleted` tag marks
  // the message.
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
  // Decoded, not merely present.
  await expect
    .poll(() => bubble.locator("img").first().evaluate((i: HTMLImageElement) => i.naturalWidth))
    .toBeGreaterThan(0);
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

test("open an IRC thread — the composer: lays out cleanly @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/irc/7");
  await page.locator(".msg .body").first().waitFor();
  // An icon-font fallback would render the word "send"; jsdom has no fonts.
  await page.getByRole("button", { name: "Send" }).waitFor();
  await expectIconFontLoaded(page);
  // A long draft crowds the row. `exact`, since the reveal button's name also
  // contains "Message".
  await page.getByLabel("Message", { exact: true }).fill(
    "a fairly long line of the sort somebody actually types on a phone, to see whether the send button survives it",
  );
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

/** The Android keyboard. `interactive-widget=resizes-content` makes it shrink
 *  the layout viewport, which is what `setViewportSize` does, so this is the real
 *  geometry. It cannot see the meta token itself; the served-page test does. */
test("the composer stays above the Android keyboard @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/irc/7");
  const input = page.getByLabel("Message", { exact: true });
  await input.waitFor();
  await input.fill("half a sentence, still being typed");

  const full = page.viewportSize()!;
  const KEYBOARD = 350; // a Pixel's, near enough
  await page.setViewportSize({ width: full.width, height: full.height - KEYBOARD });

  const box = (await input.boundingBox())!;
  expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.y + box.height).toBeLessThanOrEqual(full.height - KEYBOARD);
  const send = (await page.getByRole("button", { name: "Send" }).boundingBox())!;
  expect(send.y + send.height).toBeLessThanOrEqual(full.height - KEYBOARD);
  expect(send.x + send.width).toBeLessThanOrEqual(full.width);
  await expect(input).toHaveValue("half a sentence, still being typed");

  // Scoped to the composer: a scrolled thread pins the day pill over a message
  // by design, which a page-wide scan would count as a collision.
  await expectNoTextOverlaps(page, testInfo, ".composer");
  await expectNoHorizontalOverflow(page, testInfo);
});

/** The newest message stays visible when the keyboard opens
 *  (`ThreadWindow.observeShrink`). */
test("the newest message stays visible when the keyboard opens @ phone width", async ({ page }) => {
  await mockApi(page);
  // Registered after mockApi, so it wins.
  await page.route("**/api/conversations/**/messages**", (r) => r.fulfill({ json: LONG_THREAD }));
  await page.goto("/conversation/irc/7");
  const input = page.getByLabel("Message", { exact: true });
  await input.waitFor();
  const last = page.locator('.msg[data-id="60"]');
  const composer = page.locator(".composer");
  await last.waitFor();

  // At the latest message before the keyboard arrives.
  await expect
    .poll(() => page.locator("app-thread").evaluate((h) => h.scrollHeight - h.scrollTop - h.clientHeight))
    .toBeLessThan(64);

  const full = page.viewportSize()!;
  await input.click();
  await input.fill("replying to what is on screen");
  const KEYBOARD = 350; // a Pixel's, near enough — same figure as the test above
  await page.setViewportSize({ width: full.width, height: full.height - KEYBOARD });

  // The re-pin runs a frame after the resize.
  await expect
    .poll(() => page.locator("app-thread").evaluate((h) => h.scrollHeight - h.scrollTop - h.clientHeight))
    .toBeLessThan(64);

  // The newest message, whole, above the box and inside the viewport.
  const box = (await last.boundingBox())!;
  const comp = (await composer.boundingBox())!;
  expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.y + box.height).toBeLessThanOrEqual(comp.y + 1);
  expect(box.y + box.height).toBeLessThanOrEqual(full.height - KEYBOARD);
  await expect(input).toHaveValue("replying to what is on screen");
});

/** A reader in history is left there when the keyboard opens. */
test("a reader back in history is left there when the keyboard opens @ phone width", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) => r.fulfill({ json: LONG_THREAD }));
  await page.goto("/conversation/irc/7");
  const input = page.getByLabel("Message", { exact: true });
  await input.waitFor();
  await page.locator('.msg[data-id="60"]').waitFor();

  await page.locator("app-thread").evaluate((h) => (h.scrollTop = 0));
  await expect.poll(() => page.locator("app-thread").evaluate((h) => h.scrollTop)).toBeLessThan(50);
  const before = await page.locator("app-thread").evaluate((h) => h.scrollTop);

  const full = page.viewportSize()!;
  await input.click();
  await page.setViewportSize({ width: full.width, height: full.height - 350 });
  await page.waitForTimeout(500); // a re-pin would land within a frame; this is generous

  const after = await page.locator("app-thread").evaluate((h) => h.scrollTop);
  expect(Math.abs(after - before)).toBeLessThan(50);
  const fromBottom = await page
    .locator("app-thread")
    .evaluate((h) => h.scrollHeight - h.scrollTop - h.clientHeight);
  expect(fromBottom).toBeGreaterThan(500);
});

/** The served page carries `interactive-widget=resizes-content`; the geometry
 *  tests above resize the viewport themselves and cannot see it. */
test("the served page asks the keyboard to shrink the layout viewport", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");
  const content = await page.locator('meta[name="viewport"]').getAttribute("content");
  expect(content).toContain("interactive-widget=resizes-content");
});

/** A real IME composition (`Input.imeSetComposition`), so the Enter carries
 *  `isComposing` as Gboard's would. */
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

  // A plain Enter still sends.
  await cdp.send("Input.imeSetComposition", { text: "", selectionStart: 0, selectionEnd: 0 });
  await input.fill("a finished sentence");
  await page.keyboard.press("Enter");
  await expect.poll(() => sends).toBe(1);
});

/** Search results: two ordinary hits and a retracted one whose `snippet` still
 *  has the words, as the server sends it. */
const SEARCH = [
  { origin: "signal", conversation_id: "dm:a", conversation_name: "Alice Andersson", ts: Date.UTC(2026, 0, 2, 9, 14),
    sender: "Alice Andersson", snippet: "the referral letter finally turned up this morning, second post", deleted: false, cursor: "1000_1" },
  { origin: "signal", conversation_id: "dm:a", conversation_name: "Alice Andersson", ts: Date.UTC(2026, 0, 1, 18, 3),
    sender: "Alice Andersson", snippet: "posted the letter on Tuesday", deleted: true, cursor: "2000_2" },
  { origin: "irc", conversation_id: "7", conversation_name: "#a-channel-with-a-long-name", ts: Date.UTC(2025, 11, 30, 16, 40),
    sender: "s_20", snippet: "no letter here, wrong channel", deleted: false, cursor: "3000_3" },
  // One target on two networks: ids 8 and 9 in CONVERSATIONS.
  { origin: "irc", conversation_id: "8", conversation_name: "s_20", ts: Date.UTC(2025, 11, 29, 12, 0),
    sender: "s_20", snippet: "letter sent, check the pigeon", deleted: false, cursor: "4000_4" },
  { origin: "irc", conversation_id: "9", conversation_name: "s_20", ts: Date.UTC(2025, 11, 28, 12, 0),
    sender: "s_20", snippet: "letter never arrived", deleted: false, cursor: "5000_5" },
];

/** A retracted hit reads as a retraction among ordinary hits, without its words
 *  or crowding the row. */
test("search — a retracted hit is listed without its text @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.route("**/api/search**", (r) => r.fulfill({ json: SEARCH }));
  await page.goto("/");
  await page.getByPlaceholder("Search messages").fill("letter");
  await page.getByPlaceholder("Search messages").press("Enter");

  const list = page.locator("mat-action-list");
  await list.getByText("(deleted)").waitFor();
  await expect(page.locator("body")).not.toContainText("posted the letter on Tuesday");
  await expect(list.getByRole("button")).toHaveCount(5);
  await expect(list).toContainText("the referral letter finally turned up");

  // The two `s_20` rows are told apart by network; located by snippet.
  const rows = list.getByRole("button");
  await expect(rows.filter({ hasText: "letter sent, check the pigeon" })).toContainText("IRC xinutec");
  await expect(rows.filter({ hasText: "letter never arrived" })).toContainText("IRC euirc");

  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

// The failure state, which no other test reaches.
test("search — a failed search says so rather than \"No matches.\" @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.route("**/api/search**", (r) => r.fulfill({ status: 500, body: "boom" }));
  await page.goto("/");
  await page.getByPlaceholder("Search messages").fill("anything");
  await page.getByPlaceholder("Search messages").press("Enter");
  await page.getByText("The search didn't run", { exact: false }).waitFor();
  await expect(page.locator(".empty")).toHaveCount(0);
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});
