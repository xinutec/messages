import { expect, test, type Page } from "@playwright/test";

/**
 * Opening a conversation must land at the LATEST message (the bottom), like any
 * chat app — not at the top of the fetched page. `Thread.loadThread` calls
 * `scrollToBottom()` on open when there's no `?from`; this asserts the rendered
 * result actually sits at the bottom in a real browser at a phone viewport. Only
 * a render check can see this: jsdom has no scroll geometry (scrollHeight/
 * clientHeight are 0), so vitest can't tell "pinned to bottom" from "pinned to
 * top".
 *
 * The mock is deliberately realistic — a full newest page with a mix of short
 * and long (wrapping) bodies AND images on the newest messages. Images render
 * lazily with no reserved height, so they have zero height at first paint and
 * grow when they load *after* scrollToBottom ran; if that growth isn't handled
 * the newest messages get pushed below the fold. The test waits for the images
 * to load before measuring, so it catches exactly that.
 */

const ME = { user_id: "u1", display_name: "Test User" };
const CONVERSATIONS = [
  { origin: "signal", id: "dm:a", name: "Alice", kind: "dm", message_count: 1000, last_ts: 1_717_000_000_000 },
];

// A visible image with real dimensions (so it occupies height once loaded) —
// served for every /api/attachments/* request below.
const IMAGE_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="280" height="210"><rect width="280" height="210" fill="#3b6ea5"/></svg>`;

const LOREM =
  "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod " +
  "tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, " +
  "quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo.";

function imageAttachment(k: number) {
  return { id: `att${k}`, content_type: "image/svg+xml", file_name: `pic${k}.svg`, size: 12_345, available: true, is_image: true };
}

// One full server page (PAGE=100 in thread.ts) of the newest messages, ascending
// by ts — far taller than the 844px viewport — with older history available.
// Every body starts with `msg{k}` (so `data-id` selects them precisely); every
// 3rd is long and wraps to several lines; the last 4 carry an image. This is the
// "tap a long, media-heavy chat" case.
function newestPage(n: number) {
  const base = Date.UTC(2026, 0, 1, 12, 0, 0);
  return Array.from({ length: n }, (_, k) => ({
    id: String(k),
    ts: base + k * 60_000,
    sender: "Alice",
    is_outgoing: k % 5 === 0,
    body: k % 3 === 0 ? `msg${k} — ${LOREM}` : `msg${k}`,
    deleted: false,
    edited: false,
    reactions: [],
    attachments: k >= n - 4 ? [imageAttachment(k)] : [],
    link_images: [], link_offers: [], edits: [],
  }));
}

async function mockApi(page: Page): Promise<void> {
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
  await page.route("**/api/attachments/**", (r) => r.fulfill({ contentType: "image/svg+xml", body: IMAGE_SVG }));
  await page.route("**/api/conversations/**/messages**", async (route) => {
    const cursor = new URL(route.request().url()).searchParams.get("cursor");
    // Newest page (no cursor); opening at the bottom needs only this page.
    // Older history exists (has_more) but is fetched lazily on scroll-up.
    if (cursor) {
      await route.fulfill({ json: { messages: [], has_more: false, next_cursor: null, prev_cursor: null } });
    } else {
      await route.fulfill({
        json: { messages: newestPage(100), has_more: true, next_cursor: "1000000", prev_cursor: null },
      });
    }
  });
}

test("opening a long conversation lands at the latest message", async ({ page }) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  // The newest message (last of the page) is rendered.
  await page.locator('.msg[data-id="99"]').waitFor();
  // Wait for the images on the newest messages to finish loading — the shift
  // they cause happens AFTER the initial scrollToBottom, so measuring before
  // they load would give a false pass.
  await expect
    .poll(async () =>
      // The element type is a parameter of `evaluateAll`, so asking for
      // `HTMLImageElement` up front is what makes `.complete` and
      // `.naturalHeight` legal — an assertion inside the callback would claim
      // the same thing without anything checking it.
      page
        .locator(".attach img")
        .evaluateAll<boolean, HTMLImageElement>(
          (imgs) => imgs.length > 0 && imgs.every((i) => i.complete && i.naturalHeight > 0),
        ),
    )
    .toBe(true);
  await page.waitForTimeout(150); // let any post-load reflow settle

  const geom = await page.locator(".thread").evaluate((t) => ({
    distanceFromBottom: t.scrollHeight - t.scrollTop - t.clientHeight,
    scrollTop: t.scrollTop,
  }));
  // Pinned to the bottom (within a small epsilon), even after images grew ...
  expect(geom.distanceFromBottom).toBeLessThanOrEqual(4);
  // ... and it genuinely scrolled — proving it's a long thread that landed at the
  // end, not a short one that trivially fits (which would pass a bottom check for
  // free).
  expect(geom.scrollTop).toBeGreaterThan(0);

  // The newest bubble is actually within the viewport.
  const lastInView = await page.locator('.msg[data-id="99"]').evaluate((el) => {
    const r = el.getBoundingClientRect();
    return r.top >= 0 && r.bottom <= window.innerHeight + 1;
  });
  expect(lastInView).toBe(true);

  // And the oldest rendered bubble is scrolled off the top (it opened at the end,
  // not the start).
  const firstAboveViewport = await page
    .locator('.msg[data-id="0"]')
    .evaluate((el) => el.getBoundingClientRect().bottom < 0);
  expect(firstAboveViewport).toBe(true);
});

/**
 * Scrolling FORWARD off a search landing — #1401's other half, and the half
 * jsdom cannot see.
 *
 * `ThreadWindow.step()` decides `needNewer` from viewport rects. In jsdom every
 * rect is zero, so a unit test driving `onScroll` measures a fake — this file's
 * sibling `thread-window.spec.ts` says so and keeps the measuring half here.
 * `thread.spec.ts` covers what `fetchNewer` DOES once called; this covers the
 * one thing only a real browser can answer: that scrolling to the bottom of a
 * floating window calls it at all.
 *
 * Before #1401 the window could only grow backwards — the sole route by which
 * newer messages reached a thread was `pollNewer` asking for the newest page —
 * so a reader landed on an old hit could scroll back for ever and not forward
 * one line.
 */
function pageAt(prefix: string, startTs: number, n: number) {
  return Array.from({ length: n }, (_, k) => ({
    id: `${prefix}${k}`,
    ts: startTs + k * 60_000,
    sender: "Alice",
    is_outgoing: false,
    body: k % 3 === 0 ? `${prefix}${k} — ${LOREM}` : `${prefix}${k}`,
    deleted: false,
    edited: false,
    reactions: [],
    attachments: [], link_images: [], link_offers: [], edits: [],
  }));
}

test("scrolling to the bottom of a landing fetches forwards", async ({ page }) => {
  const HIT = Date.UTC(2005, 5, 1, 12, 0, 0);
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
  let forwardPages = 0;
  await page.route("**/api/conversations/**/messages**", async (route) => {
    const q = new URL(route.request().url()).searchParams;
    // ⚠ `at` opens the landing (inclusive of the hit) and `newer` grows it.
    // Both are counted: the first call is the landing, so a second one is proof
    // the scroll fetched forward.
    if (q.get("dir") === "newer" || q.get("dir") === "at") {
      const n = forwardPages++;
      await route.fulfill({
        json: {
          messages: pageAt(`fwd${n}_`, HIT + n * 3_600_000, 50),
          // More forward history: the window stays floating, so a second
          // scroll to the bottom must be able to ask again.
          has_more: true,
          next_cursor: null,
          prev_cursor: `c${n}`,
        },
      });
    } else {
      await route.fulfill({
        json: { messages: pageAt("old", HIT - 3_600_000, 50), has_more: true, next_cursor: "older-c", prev_cursor: null },
      });
    }
  });

  await page.goto("/conversation/signal/dm:a?at=1117627200000_9");
  await expect(page.locator('[data-id="fwd0_0"]')).toBeAttached();
  expect(forwardPages).toBe(1);

  // ⚠ **The hit is MARKED, and only the hit.** Landing on the right message is
  // not the same as showing which one: `scrollToTs` puts it flush under the
  // sticky header where, unmarked, it looks exactly like its neighbours —
  // measured on a phone against a 2013 hit in a channel where a dozen lines
  // share the minute. Asserted here rather than in vitest because the class is
  // only worth anything if it reaches the rendered DOM.
  await expect(page.locator('[data-id="fwd0_0"]')).toHaveClass(/landed/);
  await expect(page.locator('[data-id="fwd0_1"]')).not.toHaveClass(/landed/);

  // Drive the real thing: scroll the host to its bottom and let the engine
  // decide. Nothing here calls fetchNewer. `.thread` IS the scroll container —
  // the component sets it as its own host class and binds `(scroll)` there.
  //
  // ⚠ **UP, THEN DOWN, REPEATEDLY — and each half is load-bearing.** `step()`
  // reveals what the window collapsed below before it asks for more, exactly as
  // the top does, so the first arrival at the bottom spends itself on the
  // reveal. And a second `scrollTop = scrollHeight` from a position already at
  // the bottom assigns the same value, which fires NO scroll event at all:
  // measured here, ten assignments produced one event. Backing off first is
  // what makes the next arrival a real one.
  await expect
    .poll(
      async () => {
        await page.evaluate(() => {
          const t = document.querySelector(".thread");
          if (!t) return;
          t.scrollTop = Math.max(0, t.scrollTop - 400);
        });
        await page.waitForTimeout(30);
        await page.evaluate(() => {
          const t = document.querySelector(".thread");
          if (t) t.scrollTop = t.scrollHeight;
        });
        return forwardPages;
      },
      { timeout: 15_000, intervals: Array<number>(50).fill(150) },
    )
    .toBeGreaterThan(1);
});
