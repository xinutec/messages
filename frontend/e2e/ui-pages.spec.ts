import { expect, test, type Page } from "@playwright/test";
// The fleet-shared harness, @xinutec/ui-harness (~/Code/ui-harness).
import {
  expectCleanLayout,
  expectNoHorizontalOverflow,
  expectViewportIsPhone,
  expectIconFontLoaded,
  expectRecoversFromMissingBundle,
  expectUpInTheBar,
  expectBackClosesOverlay,
} from "@xinutec/ui-harness";

import type { Conversation, Me, MessagesPage, SearchHit, SendResult } from "../src/app/models";
import { testMessage } from "../src/app/test-message";

/**
 * Phone-width layout checks: the conversation list and an open thread at a
 * Pixel viewport, with the backend mocked and busy data. Asserts that no two
 * pieces of text collide and nothing spills past the right edge.
 */

const ME = { user_id: "test", display_name: "Test User" } satisfies Me;

/** A busy conversation list: every origin, a group, and a long name. */
const CONVERSATIONS = [
  { origin: "signal", id: "dm:a", name: "Alice Andersson", kind: "dm", network: null, message_count: 128, last_ts: Date.UTC(2026, 0, 2, 9, 14), last: { sender: "Alice Andersson", is_outgoing: false, deleted: false, text: "From the climbing wall on Saturday, the three of us at the top of the orange route", media: null }, unread: 3, avatar: 2 },
  { origin: "signal", id: "grp:x", name: "Saturday climbing & bouldering logistics crew", kind: "group", network: null, message_count: 4210, last_ts: Date.UTC(2026, 0, 1, 20, 2), last: { sender: "Dana", is_outgoing: false, deleted: false, text: "Six of us are in.", media: null }, unread: 128, avatar: 1 },
  { origin: "gchat", id: "gc1", name: "Bob Bytecode", kind: "dm", network: null, message_count: 37, last_ts: Date.UTC(2025, 11, 30, 16, 40), last: { sender: "Me", is_outgoing: true, deleted: false, text: "Sounds good, see you then", media: null }, unread: 0, avatar: null },
  { origin: "gchat", id: "gc2", name: "Platform on-call", kind: "group", network: null, message_count: 902, last_ts: Date.UTC(2025, 11, 29, 8, 5), last: { sender: "Erin Example", is_outgoing: false, deleted: true, text: null, media: null }, unread: 0, avatar: null },
  // IRC, the only origin with a composer.
  { origin: "irc", id: "7", name: "#a-channel-with-a-long-name", kind: "group", network: "xinutec", message_count: 5104, last_ts: Date.UTC(2026, 0, 2, 11, 30), last: { sender: "s_20", is_outgoing: false, deleted: false, text: "anyone around who knows nix flakes well enough to explain overlays?", media: null }, unread: 0, avatar: null },
  // One target on two networks, the widest subtitle.
  { origin: "irc", id: "8", name: "s_20", kind: "dm", network: "xinutec", message_count: 14446, last_ts: Date.UTC(2026, 0, 2, 10, 15), last: { sender: "s_20", is_outgoing: false, deleted: false, text: null, media: null }, unread: 0, avatar: null },
  { origin: "irc", id: "9", name: "s_20", kind: "dm", network: "euirc", message_count: 8071, last_ts: Date.UTC(2026, 0, 1, 22, 40), last: null, unread: 0, avatar: null },
] satisfies Conversation[];

/** Real bytes for the available attachment, so a revealed image actually
 *  decodes rather than rendering a broken glyph. SVG, because `Buffer` needs
 *  @types/node, which tsconfig.e2e.json lacks. */
const PIXELS =
  '<svg xmlns="http://www.w3.org/2000/svg" width="96" height="64">' +
  '<rect width="96" height="64" fill="#5a6e96"/></svg>';

/** A busy thread: every element that can crowd or overflow a bubble. */
const THREAD = {
  messages: [
    testMessage({ id: "1", ts: Date.UTC(2026, 0, 1, 12, 0), sender: "Alice Andersson", is_outgoing: false,
      body: "Morning! Did the referral letter come through yet? The clinic said they'd post it but it's been almost two weeks now.",
      deleted: false, edited: true, reply_to: null, delivery: null, entities: [], album: null, previews: [], reactions: [{ emoji: "👍", count: 3, who: ["Bob Bytecode", "Dana", "Test User"] }, { emoji: "❤️", count: 2, who: [] }, { emoji: "🎉", count: 1, who: ["Dana"] }],
      attachments: [], link_images: [], link_offers: [], edits: [] }),
    // A full-length reply excerpt on an outgoing bubble, the narrowest.
    testMessage({ id: "2", ts: Date.UTC(2026, 0, 1, 12, 4), sender: "Test User", is_outgoing: true,
      body: "Not yet — chasing them this afternoon.", deleted: false, edited: false, reactions: [],
      // Two readers, on the narrowest bubble; rendered through a DM route and a
      // group route below.
      delivery: { state: "read", read_by: [{ who: "Alice Andersson", at: Date.UTC(2026, 0, 1, 12, 6) }, { who: "Bob Bytecode", at: Date.UTC(2026, 0, 1, 12, 8) }] },
      reply_to: { id: "1", cursor: "1_1", ts: Date.UTC(2026, 0, 1, 12, 0), sender: "Alice Andersson",
        excerpt: "Morning! Did the referral letter come through yet? The clinic said they'd post it but it's been almost two weeks n…",
        deleted: false },
      attachments: [{ id: "a1", content_type: "application/pdf", file_name: "referral-scan-2026-final-v2.pdf", size: 91234, available: false, fetch: null, transcript: null },
        // Too big to fetch unasked: offered, with a long name and a size.
        { id: "a3", content_type: "video/mp4", file_name: "climbing-wall-ascent-saturday.mp4", size: 14950323, available: false, fetch: "offered", transcript: null },
        // A voice message, as Signal labels one.
        { id: "au1", content_type: "audio/aac", file_name: "signal-2026-10-10-21-27-55-970.m4a", size: 295210, available: true, fetch: null,
          transcript: "Hoi! Ben je vanavond thuis? Dan kom ik even langs met die boeken die ik nog van je heb." }],
      link_images: [], link_offers: [{ url: "https://cloud.example.org/s/holiday", id: "lo1" }], edits: [] }),
    // An unresolved quote, the longer wording.
    testMessage({ id: "3", ts: Date.UTC(2026, 0, 1, 12, 9), sender: "Alice Andersson", is_outgoing: false,
      body: "Thankyouuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuu", deleted: false, edited: false, reactions: [], delivery: null,
      reply_to: { id: null, cursor: null, ts: Date.UTC(2025, 5, 3, 8, 30), sender: null, excerpt: null, deleted: false },
      attachments: [], link_images: [], link_offers: [], edits: [] }),
    // Deleted, with words and a stored image behind the reveal.
    testMessage({ id: "4", ts: Date.UTC(2026, 0, 1, 12, 11), sender: "Alice Andersson", is_outgoing: false,
      body: "something said and then taken back", deleted: true, edited: false, reactions: [], reply_to: null, delivery: null, entities: [], album: null, previews: [],
      attachments: [{ id: "a2", content_type: "image/jpeg", file_name: null, size: 4096, available: true, fetch: null, transcript: null }], link_images: [], link_offers: [], edits: [] }),
    // A Telegram service event, rendered as an action.
    testMessage({ id: "5", ts: Date.UTC(2026, 0, 1, 12, 20), sender: "Alice Andersson", is_outgoing: false,
      kind: "action", body: "made a 55-minute video call", deleted: false, edited: false,
      reactions: [], reply_to: null, delivery: null, entities: [], album: null, previews: [],
      attachments: [], link_images: [], link_offers: [], edits: [] }),
    // Named reactors: the names are in a title and must not widen the chip.
    testMessage({ id: "6", ts: Date.UTC(2026, 0, 1, 12, 24), sender: "Alice Andersson", is_outgoing: false,
      body: "Six of us are in.", deleted: false, edited: false, reply_to: null, delivery: null, entities: [], album: null, previews: [],
      reactions: [{ emoji: "👍", count: 6, who: ["Alice Andersson", "Bob Bytecode", "Test User", "Dana", "Erin Example"] },
                  { emoji: "🎉", count: 1, who: ["Dana"] }],
      attachments: [], link_images: [], link_offers: [], edits: [] }),
    // A mention, a link preview, and a reply to a message the archive does not
    // hold whose quote carried its author and text.
    testMessage({ id: "10", ts: Date.UTC(2026, 0, 1, 12, 27), sender: "Alice Andersson", is_outgoing: false,
      body: "@Dana look: https://xinutec.org/a/rather/long/path/that/keeps/going/and/going", deleted: false, edited: false, reactions: [],
      reply_to: { id: null, cursor: null, ts: Date.UTC(2025, 5, 3, 8, 30), sender: "Bob Bytecode",
        excerpt: "Did anybody write down the address of that place with the enormous climbing wall and the good coffee?", deleted: false },
      delivery: null, entities: [{ kind: "mention", offset: 0, length: 5, url: null }], album: null,
      previews: [{ url: "https://xinutec.org/a/rather/long/path/that/keeps/going/and/going",
                   title: "A page with a title long enough to need wrapping on a phone screen",
                   description: "And a description that says a little more about what is on the page.",
                   image: "3" }],
      attachments: [], link_images: [], link_offers: [], edits: [] }),
    // An outgoing album of three: a captioned member, then two pictures.
    testMessage({ id: "7", ts: Date.UTC(2026, 0, 1, 12, 30), sender: "Test User", is_outgoing: true,
      body: "From the climbing wall on Saturday, the three of us at the top of the orange route", deleted: false, edited: false, reactions: [], reply_to: null, entities: [], album: "9007199254740993", previews: [],
      delivery: { state: "delivered", read_by: [] },
      attachments: [{ id: "p7", content_type: "image/jpeg", file_name: null, size: 4096, available: true, fetch: null, transcript: null }], link_images: [], link_offers: [], edits: [] }),
    testMessage({ id: "8", ts: Date.UTC(2026, 0, 1, 12, 30), sender: "Test User", is_outgoing: true,
      body: null, deleted: false, edited: false, reactions: [], reply_to: null, entities: [], album: "9007199254740993", previews: [],
      delivery: { state: "delivered", read_by: [] },
      attachments: [{ id: "p8", content_type: "image/jpeg", file_name: null, size: 4096, available: true, fetch: null, transcript: null }], link_images: [], link_offers: [], edits: [] }),
    testMessage({ id: "9", ts: Date.UTC(2026, 0, 1, 12, 30), sender: "Test User", is_outgoing: true,
      body: null, deleted: false, edited: false, reactions: [], reply_to: null, entities: [], album: "9007199254740993", previews: [],
      delivery: { state: "delivered", read_by: [] },
      attachments: [{ id: "p9", content_type: "image/jpeg", file_name: null, size: 4096, available: true, fetch: null, transcript: null }], link_images: [], link_offers: [], edits: [] }),
    // An incoming album of four, captioned by its second member: the even case,
    // where every row of the grid is full. The last is a video, as albums from a phone mix them.
    testMessage({ id: "21", ts: Date.UTC(2026, 0, 1, 12, 35), sender: "Alice Andersson", is_outgoing: false,
      body: null, deleted: false, edited: false, reactions: [], reply_to: null, entities: [], album: "9007199254740994", previews: [],
      delivery: null,
      attachments: [{ id: "p21", content_type: "image/jpeg", file_name: null, size: 4096, available: true, fetch: null, transcript: null }], link_images: [], link_offers: [], edits: [] }),
    testMessage({ id: "22", ts: Date.UTC(2026, 0, 1, 12, 35), sender: "Alice Andersson", is_outgoing: false,
      body: "Four from the summit, before the rain came in", deleted: false, edited: false, reactions: [], reply_to: null, entities: [], album: "9007199254740994", previews: [],
      delivery: null,
      attachments: [{ id: "p22", content_type: "image/jpeg", file_name: null, size: 4096, available: true, fetch: null, transcript: null }], link_images: [], link_offers: [], edits: [] }),
    testMessage({ id: "23", ts: Date.UTC(2026, 0, 1, 12, 35), sender: "Alice Andersson", is_outgoing: false,
      body: null, deleted: false, edited: false, reactions: [], reply_to: null, entities: [], album: "9007199254740994", previews: [],
      delivery: null,
      attachments: [{ id: "p23", content_type: "image/jpeg", file_name: null, size: 4096, available: true, fetch: null, transcript: null }], link_images: [], link_offers: [], edits: [] }),
    testMessage({ id: "24", ts: Date.UTC(2026, 0, 1, 12, 35), sender: "Alice Andersson", is_outgoing: false,
      body: null, deleted: false, edited: false, reactions: [], reply_to: null, entities: [], album: "9007199254740994", previews: [],
      delivery: null,
      attachments: [{ id: "v24", content_type: "video/webm", file_name: null, size: 10832, available: true, fetch: null, transcript: null }], link_images: [], link_offers: [], edits: [] }),
  ],
  has_more: false,
  next_cursor: null,
  prev_cursor: null,
} satisfies MessagesPage;

/** A thread taller than the pane, so there is a scroll position the keyboard
 *  can disturb. */
const LONG_THREAD = {
  messages: Array.from({ length: 60 }, (_, i) =>
    testMessage({
      id: String(i + 1),
      ts: Date.UTC(2026, 0, 2, 10, 0) + i * 60_000,
      sender: i % 2 ? "Test User" : "Alice Andersson",
      is_outgoing: i % 2 === 1,
      body: `line number ${i + 1} of the conversation`,
      // Every delivery rung is laid out; `delivered` is the longest word.
      delivery:
        i % 2 === 1 ? { state: (["sent", "delivered", "read"] as const)[(i >> 1) % 3], read_by: [] } : null,
    }),
  ),
  has_more: false,
  next_cursor: null,
  prev_cursor: null,
} satisfies MessagesPage;

/** Mock every backend call. The catch-all goes first: Playwright runs handlers
 *  last-registered-first. */
async function mockApi(page: Page): Promise<void> {
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
  await page.route("**/api/conversations/**/messages**", (r) => r.fulfill({ json: THREAD }));
  // See PIXELS.
  await page.route(/\/api\/(attachments|link-previews)\//, (r) =>
    r.fulfill({ contentType: "image/svg+xml", body: PIXELS }),
  );
  // The group's picture; Alice's is gone, and the row must fall back to initials.
  await page.route("**/api/conversations/signal/grp%3Ax/avatar?v=1", (r) =>
    r.fulfill({ contentType: "image/svg+xml", body: PIXELS }),
  );
  await page.route("**/api/conversations/signal/dm%3Aa/avatar?v=2", (r) => r.fulfill({ status: 404 }));
  // A second of silence, as WAV: Playwright's Chromium may not play AAC.
  await page.route("**/api/attachments/au1", (r) =>
    r.fulfill({ contentType: "audio/wav", path: "e2e/beep.wav" }),
  );
  // A second of test pattern, in WebM: Playwright's Chromium plays no H.264.
  await page.route("**/api/attachments/v24", (r) =>
    r.fulfill({ contentType: "video/webm", path: "e2e/clip.webm" }),
  );
}

// Fails if the device preset is lost and the suite runs at desktop width.
test("the suite really runs at phone geometry", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");
  await expectViewportIsPhone(page);
});

// A service worker can serve an index naming a bundle a later deploy removed, and
// the app's own update handling is inside that bundle (#1823). The recovery is
// inline in `src/index.html`; this checks it is there and works.
test("a bundle a deploy removed reloads into the app, not a blank screen", async ({ page }) => {
  await mockApi(page);
  await expectRecoversFromMissingBundle(page, "/", 'input[placeholder="Search messages"]');
});

test("conversation list — filter row + rows: lays out cleanly @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/");
  await page.getByPlaceholder("Search messages").waitFor();
  await page.getByRole("radio", { name: "Google Chat", exact: true }).waitFor(); // widest filter toggle
  await page.getByText("Alice Andersson").waitFor();
  // By the name alone: a preview can carry another row's name as its sender.
  const row = (name: string) =>
    page.locator("button.row", { has: page.locator(".name", { hasText: new RegExp(`^${name}`) }) });
  // Two rows titled `s_20`; only the network separates them.
  await expect(row("s_20").locator(".net")).toHaveText(["xinutec", "euirc"]);
  // The newest message: the sender in a group, You for mine, none in a DM.
  // Visible, not only present: Material clips a line its layout has no room for.
  await expect(row("Saturday climbing").locator(".preview")).toBeVisible();
  await expect(row("Saturday climbing").locator(".preview")).toHaveText("Dana: Six of us are in.");
  await expect(row("Bob Bytecode").locator(".preview")).toHaveText("You: Sounds good, see you then");
  await expect(row("Alice Andersson").locator(".preview")).toContainText(/^From the climbing wall/);
  await expect(row("Platform on-call").locator(".quiet")).toHaveText("Erin Example: Message deleted");
  // A picture where the origin keeps one; initials where it does not, or where
  // the picture would not load.
  const picture = row("Saturday climbing").locator(".avatar img");
  await expect
    .poll(() => picture.evaluate((e) => e instanceof HTMLImageElement && e.naturalWidth > 0))
    .toBe(true);
  await expect(row("Saturday climbing").locator(".avatar")).not.toContainText("SC");
  await expect(row("Alice Andersson").locator(".avatar")).toContainText("AA");
  await expect(row("Alice Andersson").locator(".avatar img")).toHaveCount(0);
  await expect(row("Bob Bytecode").locator(".avatar")).toContainText("BB");
  // Unread on the phone: a count, capped; none where nothing is unread.
  await expect(row("Alice Andersson").locator(".badge")).toHaveText("3");
  await expect(row("Saturday climbing").locator(".badge")).toHaveText("99+");
  await expect(row("Bob Bytecode").locator(".badge")).toHaveCount(0);
  // Whole and round: it once sat clipped in a slot sized for one line of text.
  const rowBox = (await row("Alice Andersson").boundingBox())!;
  const badge = (await row("Alice Andersson").locator(".badge").boundingBox())!;
  expect(badge.y + badge.height).toBeLessThanOrEqual(rowBox.y + rowBox.height);
  expect(badge.width).toBeCloseTo(badge.height, 0);
  await page.screenshot({ path: testInfo.outputPath("list.png") });
  // An icon-font fallback would show the word "search" over the placeholder.
  await expectIconFontLoaded(page);
  await expectCleanLayout(page, testInfo);
});

test("open thread — meta + reactions + attachment: lays out cleanly @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  // Wait for the far messages, so the thread has laid out.
  await page.locator(".msg .body").first().waitFor();
  await page.getByText("👍 3").waitFor();
  await page.getByText("referral-scan-2026-final-v2.pdf", { exact: false }).waitFor();
  // The bar leads with up, named for the conversation (`@xinutec/ui-scaffold`).
  await expectUpInTheBar(page);
  await expect(page.locator("ui-scaffold h1")).toHaveText("Alice Andersson");
  // Who it is, before the name, as in the list: Alice's picture is gone, so her initials.
  await expect(page.locator("ui-scaffold app-avatar")).toContainText("AA");
  // The offers are Material's text buttons, laid out with the rest.
  const offers = page.locator(".msg button[matButton]");
  await expect(offers).toHaveText([/Show picture/, /climbing-wall-ascent-saturday\.mp4 +\(14 MB\)/]);
  // A voice message plays in place: the player is there, and has what it needs.
  const voice = page.locator(".msg audio");
  await expect(voice).toHaveAttribute("src", "/api/attachments/au1");
  await expect
    .poll(() => voice.evaluate((e) => e instanceof HTMLAudioElement && e.readyState >= 1))
    .toBe(true);
  // What the transcriber heard, under it and marked as such.
  await expect(page.locator(".msg .transcript")).toContainText("Transcribed");
  await expect(page.locator(".msg .transcript")).toContainText("Ben je vanavond thuis?");
  await offers.first().scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath("offers.png") });
  await expectCleanLayout(page, testInfo);
});

// A mention is its own run, a link preview renders as a card with its picture,
// and an unresolved quote shows what it quoted.
test("a link preview and a quoted message from before the archive @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  const bubble = page.locator('.msg[data-id="10"]');
  await expect(bubble.locator(".body .fmt-mention")).toHaveText("@Dana");
  // The address Signal sent as plain text is a link, opened outside the app.
  const link = bubble.locator(".body a");
  await expect(link).toHaveAttribute("href", "https://xinutec.org/a/rather/long/path/that/keeps/going/and/going");
  await expect(link).toHaveAttribute("target", "_blank");
  await expect(bubble.locator(".preview .title")).toHaveText(
    "A page with a title long enough to need wrapping on a phone screen");
  await expect(bubble.locator(".preview .host")).toHaveText("xinutec.org");
  await expect(bubble.locator(".preview img")).toHaveAttribute("src", "/api/link-previews/3/image");
  await expect
    .poll(() => bubble.locator(".preview img").evaluate((e) => e instanceof HTMLImageElement && e.naturalWidth > 0))
    .toBe(true);
  await expect(bubble.locator(".reply-quote .who")).toHaveText("Bob Bytecode");
  // In the accent, not the text's colour: the name has no sender hue, and once
  // lost the accent with it.
  const quoteName = await bubble.locator(".reply-quote .who").evaluate((e) => getComputedStyle(e).color);
  expect(quoteName).not.toBe(await bubble.locator(".reply-quote .said").evaluate((e) => getComputedStyle(e).color));
  await expect(bubble.locator(".reply-quote .said")).toContainText("enormous climbing wall");
  await bubble.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath("preview.png") });
  await expectCleanLayout(page, testInfo);
});

// An album renders as one set: its pictures fill a two-column grid, with the
// caption and one meta line under them. Three is the odd case and four the even
// one; once the captioned member took a row of its own, four came out with two
// half-empty rows.
// In a group each sender's name has a colour of its own, keyed by what outlasts
// a rename: Dana renamed keeps hers.
test("a sender's name keeps its colour through a rename @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  const said = (id: string, sender: string, key: string) =>
    testMessage({ id, ts: Date.UTC(2026, 0, 1, 9, Number(id)), sender, sender_key: key, body: `${sender} speaking` });
  await page.route("**/api/conversations/signal/grp%3Ax/messages**", (r) =>
    r.fulfill({
      json: {
        messages: [said("1", "Dana", "u-dana"), said("2", "Erin Example", "u-erin"), said("3", "Dana the Bold", "u-dana")],
        has_more: false, next_cursor: null, prev_cursor: null,
      } satisfies MessagesPage,
    }),
  );
  await page.goto("/conversation/signal/grp%3Ax");
  const colour = (name: string) =>
    page.locator(".msg .who", { hasText: new RegExp(`^${name}$`) }).evaluate((e) => getComputedStyle(e).color);
  await page.getByText("Dana the Bold speaking").waitFor();
  expect(await colour("Dana")).toBe(await colour("Dana the Bold"));
  expect(await colour("Dana")).not.toBe(await colour("Erin Example"));
  await page.screenshot({ path: testInfo.outputPath("senders.png") });
  await expectCleanLayout(page, testInfo);
});

test("an album is one set under one meta line @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  const albums = page.locator(".run.album");
  await expect(albums).toHaveCount(2);
  const sets: [number, string][] = [
    [3, "From the climbing wall"],
    [4, "Four from the summit"],
  ];
  for (const [i, [pictures, captioned]] of sets.entries()) {
    const album = albums.nth(i);
    await album.scrollIntoViewIfNeeded();
    await expect(album.locator(".msg")).toHaveCount(pictures);
    await expect(album.locator(".meta:visible")).toHaveCount(1);
    // Every tile drawn: a picture loaded, a video with a frame to show before
    // play (`readyState` 2 is HAVE_CURRENT_DATA).
    await expect
      .poll(() => album.locator("img, video").evaluateAll((els) =>
        els.filter((e) => e instanceof HTMLImageElement ? e.complete && e.naturalWidth > 0
          : e instanceof HTMLVideoElement && e.readyState >= 2).length))
      .toBe(pictures);
    const albumWidth = (await album.boundingBox())!.width;
    const boxes = await album.locator("img, video").evaluateAll((els) => els.map((e) => e.getBoundingClientRect()).map((r) => ({ w: r.width, top: r.top, bottom: r.bottom })));
    // As few rows as the pictures need: the captioned one once took a row of
    // its own, and four came out as [1 2] [3 _] [4 _].
    expect(new Set(boxes.map((b) => Math.round(b.top))).size).toBe(Math.ceil(pictures / 2));
    // Each picture fills its cell: a picture button sized to its picture once
    // left them at their own 96px.
    for (const b of boxes) expect(b.w).toBeGreaterThan(albumWidth * 0.4);
    // The caption comes after every picture, whichever member carried it.
    const caption = (await album.getByText(captioned).boundingBox())!;
    expect(caption.y).toBeGreaterThanOrEqual(Math.max(...boxes.map((b) => b.bottom)) - 1);
    await page.screenshot({ path: testInfo.outputPath(`album-${pictures}.png`) });
  }
  await expectCleanLayout(page, testInfo);
});

// A picture opens in the fleet's viewer over the thread, and back closes it
// with the thread where it was.
test("a picture opens in the viewer and back closes it @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  const picture = page.locator(".attach .picture").first();
  await picture.scrollIntoViewIfNeeded();
  await expectBackClosesOverlay(page, async () => {
    await picture.click();
    const img = page.locator("ui-picture-sheet img");
    await expect(img).toHaveAttribute("src", /^\/api\/attachments\//);
    // Settled: the sheet has slid up to fill the screen, and the picture is drawn.
    const viewport = page.viewportSize()!;
    await expect
      .poll(async () => (await page.locator(".ui-picture-panel").boundingBox())?.height)
      .toBeCloseTo(viewport.height, 0);
    await expect.poll(() => img.evaluate((e) => (e instanceof HTMLImageElement ? e.naturalWidth : 0))).toBeGreaterThan(0);
    // The pane is full height at once; the sheet inside it slides up into it.
    await page.locator("ui-picture-sheet").evaluate(async (e) => {
      const sheet = e.closest(".cdk-overlay-pane") ?? e;
      await Promise.all(sheet.getAnimations({ subtree: true }).map((a) => a.finished));
    });
    await page.screenshot({ path: testInfo.outputPath("viewer.png") });
    await expectCleanLayout(page, testInfo, { root: "ui-picture-sheet" });
  });
});

// Who reacted lives in a `title`, which cannot change the chip's size, and
// `count` stays the number.
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

// Two receipts are everybody in a DM, and two people in a group.
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
  await expectCleanLayout(page, testInfo);
});

// The shell's search box is hidden at phone width with a thread open, so the
// thread carries its own.
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
      ] satisfies SearchHit[],
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
  await expect(panel.getByText("Test User: Message deleted")).toBeVisible();
  await expect(panel.getByText("something withdrawn")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("thread-search.png") });

  // Scoped to the panel: a day header pins behind it, which a geometric scan
  // cannot tell from a collision.
  await expectCleanLayout(page, testInfo, { root: ".thread-search" });
  await expectNoHorizontalOverflow(page, testInfo);
});

// "No matches" is a claim, so a failed request must not make it.
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

/** Pick the first day of this month on the bar's calendar: on the calendar as
 *  it opens, and never after today. */
async function pickFirstOfMonth(page: Page): Promise<number> {
  // In the browser: its local midnight is what the app sends, and the runner's
  // time zone (UTC in CI) is not the browser's.
  const { label, ms } = await page.evaluate(() => {
    const now = new Date();
    const day = new Date(now.getFullYear(), now.getMonth(), 1);
    return {
      label: day.toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" }),
      ms: day.getTime(),
    };
  });
  await page.getByRole("button", { name: "Jump to a date" }).click();
  await page.locator("mat-calendar").getByRole("button", { name: label, exact: true }).click();
  return ms;
}

// The day goes to the server as local midnight in milliseconds; the server
// converts it to the origin's unit.
test("picking a date asks the server for that day @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  const asked: string[] = [];
  await page.route("**/messages**", (r) => {
    asked.push(r.request().url());
    return r.fulfill({ json: THREAD });
  });
  await page.goto("/conversation/signal/dm:a");
  await page.locator(".msg .body").first().waitFor();

  await page.getByRole("button", { name: "Jump to a date" }).click();
  await expect(page.locator("mat-calendar")).toBeVisible();
  // Once the menu has faded in.
  await page.waitForTimeout(400);
  await page.screenshot({ path: testInfo.outputPath("calendar.png") });
  await page.keyboard.press("Escape");
  const expected = String(await pickFirstOfMonth(page));
  // A pick closes the calendar.
  await expect(page.locator("mat-calendar")).toHaveCount(0);
  await expect.poll(() => page.url()).toContain(`on=${expected}`);
  // Both halves of the landing carry it.
  const withOn = asked.filter((u) => u.includes(`on=${expected}`));
  expect(withOn.some((u) => u.includes("dir=older"))).toBe(true);
  expect(withOn.some((u) => u.includes("dir=at"))).toBe(true);

  await expectCleanLayout(page, testInfo);
});

/** A page from a picked day: thirty messages, which no other page holds. */
async function mockDay(page: Page): Promise<void> {
  await page.route("**/messages**", (r) => {
    const url = r.request().url();
    if (!url.includes("on=")) return r.fulfill({ json: THREAD });
    // Enough to scroll within, so a scroll leaves a `?from`.
    const day = url.includes("dir=at")
      ? Array.from({ length: 30 }, (_, n) =>
          testMessage({ id: `d${n}`, ts: Date.UTC(2025, 2, 4, 9, n), sender: "Alice Andersson", body: n === 0 ? "a message from that day" : `later that day ${n}` }))
      : [];
    return r.fulfill({ json: { messages: day, has_more: false, next_cursor: null, prev_cursor: null } satisfies MessagesPage });
  });
}

// A jump is a move within the conversation, so it takes the place of where the
// reader was: back leaves the conversation, as it would from anywhere in it.
test("back after picking a date leaves the conversation @ phone width", async ({ page }) => {
  await mockApi(page);
  await mockDay(page);
  await page.goto("/");
  await page.getByText("Alice Andersson").first().click();
  await page.locator(".msg .body").first().waitFor();
  await pickFirstOfMonth(page);
  await page.getByText("a message from that day").waitFor();
  await page.goBack();
  await expect.poll(() => new URL(page.url()).pathname).toBe("/");
});

// Once the reader moves on from a picked day, the address says where they are,
// not the day: a reload or a reopen must not land there again. And the same day
// can be picked again.
test("scrolling after picking a date drops it, and it can be picked again @ phone width", async ({ page }) => {
  await mockApi(page);
  await mockDay(page);
  await page.goto("/conversation/signal/dm:a");
  await page.locator(".msg .body").first().waitFor();
  const day = await pickFirstOfMonth(page);
  await page.getByText("a message from that day").waitFor();
  await page.locator("app-thread").hover();
  await page.mouse.wheel(0, -400);
  await expect.poll(() => page.url(), { timeout: 5000 }).not.toContain("on=");
  await pickFirstOfMonth(page);
  await expect.poll(() => page.url(), { timeout: 5000 }).toContain(`on=${day}`);
});

// Pages the app loads around a landing by itself are not the reader moving: the
// day and its marker stay until the reader scrolls. On a phone the landing
// loads them at once, and the marker was gone before it was seen.
test("a picked day stays picked while the app loads around it @ phone width", async ({ page }) => {
  await mockApi(page);
  const around = (id: string, minute: number, body: string) =>
    testMessage({ id, ts: Date.UTC(2025, 2, 4, 8, minute), sender: "Alice Andersson", body });
  await page.route("**/messages**", (r) => {
    const url = r.request().url();
    if (url.includes("cursor=c1")) {
      return r.fulfill({ json: { messages: Array.from({ length: 30 }, (_, n) => around(`o${n}`, n, `earlier ${n}`)), has_more: false, next_cursor: null, prev_cursor: null } satisfies MessagesPage });
    }
    if (!url.includes("on=")) return r.fulfill({ json: THREAD });
    // The landing: the day's message, with older ones the window will want.
    return url.includes("dir=at")
      ? r.fulfill({ json: { messages: [around("d1", 59, "a message from that day")], has_more: false, next_cursor: null, prev_cursor: null } satisfies MessagesPage })
      : r.fulfill({ json: { messages: [], has_more: true, next_cursor: "c1", prev_cursor: null } satisfies MessagesPage });
  });
  await page.goto("/conversation/signal/dm:a");
  await page.locator(".msg .body").first().waitFor();
  await pickFirstOfMonth(page);
  await page.getByText("earlier 0", { exact: true }).waitFor();
  await page.waitForTimeout(800);
  expect(page.url()).toContain("on=");
  await expect(page.locator(".msg.landed")).toHaveCount(1);
});

// A hit in the open conversation is a landing like a picked day: it stays landed
// while the app loads around it, and as a move within the conversation it takes
// the reader's place, so back leaves the conversation.
test("a hit in the open conversation lands, stays landed, and back leaves @ phone width", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/search**", (r) =>
    r.fulfill({
      json: [{ origin: "signal", conversation_id: "dm:a", conversation_name: "Alice Andersson",
        ts: Date.UTC(2025, 2, 4, 9, 0), sender: "Alice Andersson",
        snippet: "the hit itself", deleted: false, cursor: "c1" }] satisfies SearchHit[],
    }),
  );
  const said = (id: string, minute: number, body: string) =>
    testMessage({ id, ts: Date.UTC(2025, 2, 4, 8, minute), sender: "Alice Andersson", body });
  await page.route("**/messages**", (r) => {
    const url = r.request().url();
    const pageOf = (messages: ReturnType<typeof said>[], more: boolean, next: string | null) =>
      r.fulfill({ json: { messages, has_more: more, next_cursor: next, prev_cursor: null } satisfies MessagesPage });
    if (url.includes("cursor=c0")) return pageOf(Array.from({ length: 30 }, (_, n) => said(`o${n}`, n, `earlier ${n}`)), false, null);
    if (!url.includes("cursor=c1")) return r.fulfill({ json: THREAD });
    // The landing: the hit, with older pages the window will want at once.
    return url.includes("dir=at") ? pageOf([said("h1", 59, "the hit itself")], false, null) : pageOf([], true, "c0");
  });
  await page.goto("/");
  await page.getByText("Alice Andersson").first().click();
  await page.locator(".msg .body").first().waitFor();
  await page.getByRole("button", { name: "Search this conversation" }).click();
  const box = page.getByPlaceholder("Search this conversation");
  await box.fill("hit");
  await box.press("Enter");
  await page.locator(".thread-search").getByText("the hit itself").click();
  await page.getByText("earlier 0", { exact: true }).waitFor();
  await page.waitForTimeout(800);
  expect(page.url()).toContain("at=c1");
  await expect(page.locator(".msg.landed")).toHaveCount(1);
  await page.goBack();
  await expect.poll(() => new URL(page.url()).pathname).toBe("/");
});

// Formatting renders from the body: the visible text is exactly what was sent.
test("telegram formatting renders from the body, never from the entity @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  const body = "👋 bold here, a link https://example.com/x and a secret plus unknownfmt";
  await page.route("**/messages**", (r) =>
    r.fulfill({
      json: {
        messages: [testMessage({
          id: "1", ts: Date.UTC(2026, 0, 1, 12, 0), sender: "Alice Andersson", body,
          entities: [
            // UTF-16 units: the leading 👋 is two, so "bold" starts at 3.
            { kind: "bold", offset: 3, length: 4, url: null },
            { kind: "url", offset: 21, length: 21, url: null },
            { kind: "spoiler", offset: 49, length: 6, url: null },
            // An unknown kind renders as plain text.
            { kind: "someFutureKind", offset: 61, length: 10, url: null },
          ],
        })],
        has_more: false, next_cursor: null, prev_cursor: null,
      } satisfies MessagesPage,
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
  await expectCleanLayout(page, testInfo);
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
  await expectCleanLayout(page, testInfo);
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
  await expectCleanLayout(page, testInfo);
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
  await page.screenshot({ path: testInfo.outputPath("composer.png") });
  await expectCleanLayout(page, testInfo);
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
  await expectCleanLayout(page, testInfo, { root: ".composer" });
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
    await r.fulfill({ json: { sent: true, error: null, archived: true } satisfies SendResult });
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
] satisfies SearchHit[];

/** A retracted hit reads as a retraction among ordinary hits, without its words
 *  or crowding the row. */
test("search — a retracted hit is listed without its text @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.route("**/api/search**", (r) => r.fulfill({ json: SEARCH }));
  await page.goto("/");
  await page.getByPlaceholder("Search messages").fill("letter");
  await page.getByPlaceholder("Search messages").press("Enter");

  const list = page.locator(".rows");
  await list.getByText("Message deleted").waitFor();
  await expect(page.locator("body")).not.toContainText("posted the letter on Tuesday");
  await expect(list.getByRole("button")).toHaveCount(5);
  await expect(list).toContainText("the referral letter finally turned up");

  // The two `s_20` rows are told apart by network; located by snippet.
  const rows = list.getByRole("button");
  await expect(rows.filter({ hasText: "letter sent, check the pigeon" }).locator(".net")).toHaveText("xinutec");
  await expect(rows.filter({ hasText: "letter never arrived" }).locator(".net")).toHaveText("euirc");
  await page.screenshot({ path: testInfo.outputPath("search.png") });

  await expectCleanLayout(page, testInfo);
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
  await expectCleanLayout(page, testInfo);
});
