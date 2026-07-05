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
  }));
}

async function mockApi(page: Page): Promise<void> {
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
  await page.route("**/api/attachments/**", (r) => r.fulfill({ contentType: "image/svg+xml", body: IMAGE_SVG }));
  await page.route("**/api/conversations/**/messages**", (route) => {
    const cursor = new URL(route.request().url()).searchParams.get("cursor");
    // Newest page (no cursor); opening at the bottom needs only this page.
    // Older history exists (has_more) but is fetched lazily on scroll-up.
    if (cursor) {
      route.fulfill({ json: { messages: [], has_more: false, next_cursor: null } });
    } else {
      route.fulfill({ json: { messages: newestPage(100), has_more: true, next_cursor: "1000000" } });
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
      page.locator(".attach img").evaluateAll((imgs) =>
        imgs.length > 0 && imgs.every((i) => (i as HTMLImageElement).complete && (i as HTMLImageElement).naturalHeight > 0),
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
