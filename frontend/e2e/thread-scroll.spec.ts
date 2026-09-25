import { expect, test, type Page } from "@playwright/test";

import type { Attachment } from "../src/app/models";
import { testMessage } from "../src/app/test-message";

/**
 * Opening a conversation lands at the latest message, even after lazy images
 * on the newest messages load and grow. jsdom has no scroll geometry, so only a
 * real browser can check this.
 */

const ME = { user_id: "u1", display_name: "Test User" };
const CONVERSATIONS = [
  { origin: "signal", id: "dm:a", name: "Alice", kind: "dm", message_count: 1000, last_ts: 1_717_000_000_000 },
];

// A real image, served for every /api/attachments/* request.
const IMAGE_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="280" height="210"><rect width="280" height="210" fill="#3b6ea5"/></svg>`;

const LOREM =
  "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod " +
  "tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, " +
  "quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo.";

function imageAttachment(k: number): Attachment {
  return { id: `att${k}`, content_type: "image/svg+xml", file_name: `pic${k}.svg`, size: 12_345, available: true, is_image: true, fetch: null };
}

// One full page of the newest messages, far taller than the viewport, with
// older history available. Bodies start `msg{k}`; every third wraps; the last
// four carry an image.
function newestPage(n: number) {
  const base = Date.UTC(2026, 0, 1, 12, 0, 0);
  return Array.from({ length: n }, (_, k) =>
    testMessage({
      id: String(k),
      ts: base + k * 60_000,
      sender: "Alice",
      is_outgoing: k % 5 === 0,
      body: k % 3 === 0 ? `msg${k} — ${LOREM}` : `msg${k}`,
      attachments: k >= n - 4 ? [imageAttachment(k)] : [],
    }),
  );
}

async function mockApi(page: Page): Promise<void> {
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
  await page.route("**/api/attachments/**", (r) => r.fulfill({ contentType: "image/svg+xml", body: IMAGE_SVG }));
  await page.route("**/api/conversations/**/messages**", async (route) => {
    const cursor = new URL(route.request().url()).searchParams.get("cursor");
    // The newest page; older history is fetched on scroll-up.
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
  await page.locator('.msg[data-id="99"]').waitFor();
  // Wait for the images, whose growth comes after the initial scroll.
  await expect
    .poll(async () =>
      // `instanceof` narrows to `HTMLImageElement`, and fails on anything else.
      page
        .locator(".attach img")
        .evaluateAll(
          (imgs) =>
            imgs.length > 0 &&
            imgs.every((i) => i instanceof HTMLImageElement && i.complete && i.naturalHeight > 0),
        ),
    )
    .toBe(true);
  await page.waitForTimeout(150); // let any post-load reflow settle

  const geom = await page.locator(".thread").evaluate((t) => ({
    distanceFromBottom: t.scrollHeight - t.scrollTop - t.clientHeight,
    scrollTop: t.scrollTop,
  }));
  // At the bottom, after the images grew,
  expect(geom.distanceFromBottom).toBeLessThanOrEqual(4);
  // and it scrolled: a long thread, not one that fits.
  expect(geom.scrollTop).toBeGreaterThan(0);

  const lastInView = await page.locator('.msg[data-id="99"]').evaluate((el) => {
    const r = el.getBoundingClientRect();
    return r.top >= 0 && r.bottom <= window.innerHeight + 1;
  });
  expect(lastInView).toBe(true);

  // The oldest rendered bubble is off the top.
  const firstAboveViewport = await page
    .locator('.msg[data-id="0"]')
    .evaluate((el) => el.getBoundingClientRect().bottom < 0);
  expect(firstAboveViewport).toBe(true);
});

/**
 * Scrolling forward off a search landing. `step()` decides `needNewer` from
 * viewport rects, which are zero in jsdom, so only a browser can show that
 * scrolling to the bottom of a floating window fetches forward.
 */
function pageAt(prefix: string, startTs: number, n: number) {
  return Array.from({ length: n }, (_, k) =>
    testMessage({
      id: `${prefix}${k}`,
      ts: startTs + k * 60_000,
      sender: "Alice",
      body: k % 3 === 0 ? `${prefix}${k} — ${LOREM}` : `${prefix}${k}`,
    }),
  );
}

test("scrolling to the bottom of a landing fetches forwards", async ({ page }) => {
  const HIT = Date.UTC(2005, 5, 1, 12, 0, 0);
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
  let forwardPages = 0;
  await page.route("**/api/conversations/**/messages**", async (route) => {
    const q = new URL(route.request().url()).searchParams;
    // `at` opens the landing and `newer` grows it; a second call proves the
    // scroll fetched forward.
    if (q.get("dir") === "newer" || q.get("dir") === "at") {
      const n = forwardPages++;
      await route.fulfill({
        json: {
          messages: pageAt(`fwd${n}_`, HIT + n * 3_600_000, 50),
          // More history ahead, so the window stays floating.
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

  // The hit, and only the hit, is marked in the rendered DOM.
  await expect(page.locator('[data-id="fwd0_0"]')).toHaveClass(/landed/);
  await expect(page.locator('[data-id="fwd0_1"]')).not.toHaveClass(/landed/);

  // Scroll the host (`.thread`) to its bottom and let the engine decide. Up,
  // then down, repeatedly: the first arrival spends itself revealing collapsed
  // rows, and assigning the same scrollTop again fires no scroll event.
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
