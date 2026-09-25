import { expect, test, type Page } from "@playwright/test";
import { expectIconFontLoaded, expectNoTextOverlaps } from "@xinutec/ui-harness";

import { testMessage } from "../src/app/test-message";

/**
 * The authenticated shell at a phone viewport, backend mocked: the Material
 * Icons font loads (no ligature words) and no text collides. jsdom sees neither.
 */

/** Scroll `.thread` to `top`, a pixel offset or "bottom", failing if there is
 *  nothing to scroll: both callers assert what is on screen afterwards. */
async function scrollThread(page: Page, top: number | "bottom"): Promise<void> {
  await page.locator(".thread").waitFor();
  // Reapplied until it holds for three frames: opening a thread jumps to the
  // newest message, possibly after this first runs.
  const settled = await page.evaluate(async (to) => {
    const t = document.querySelector(".thread");
    if (!t) return null;
    // scrollTop can reach only scrollHeight minus the visible height.
    const want = () => (to === "bottom" ? t.scrollHeight - t.clientHeight : to);
    const frame = () => new Promise((r) => requestAnimationFrame(() => r(null)));
    let held = 0;
    for (let i = 0; i < 120 && held < 3; i++) {
      if (Math.abs(t.scrollTop - want()) > 2) {
        t.scrollTop = want();
        held = 0;
      } else {
        held++;
      }
      await frame();
    }
    return { at: t.scrollTop, want: want(), held };
  }, top);
  if (settled === null) {
    throw new Error("scrollThread: .thread is not in the DOM — nothing was scrolled");
  }
  if (settled.held < 3) {
    throw new Error(
      `scrollThread: asked for ${settled.want}, left at ${settled.at} — something keeps scrolling it`,
    );
  }
}

const ME = { user_id: "u1", display_name: "Test User" };

const CONVERSATIONS = [
  { origin: "signal", id: "dm:a", name: "Alice", kind: "dm", message_count: 5, last_ts: 1_717_000_000_000 },
  { origin: "gchat", id: "gc1", name: "Bob", kind: "dm", message_count: 3, last_ts: 1_717_100_000_000 },
];

/** Mock every backend call. The catch-all goes first: Playwright runs handlers
 *  last-registered-first. */
async function mockApi(page: Page): Promise<void> {
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
}

test("authenticated shell renders: icon font loaded, no text overlaps @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/");
  // The search field's prefix icon.
  await page.getByPlaceholder("Search messages").waitFor();
  await page.getByText("Alice").waitFor();
  await expectIconFontLoaded(page);
  await expectNoTextOverlaps(page, testInfo);
});

// Two days, each taller than the viewport, so a day header pins while
// scrolling within it.
function multiDayThread() {
  const base = Date.UTC(2026, 0, 1, 12, 0, 0); // Jan 1 (Thu), Jan 2 (Fri)
  const out = [];
  for (let d = 0; d < 2; d++) {
    for (let k = 0; k < 25; k++) {
      const ts = base + d * 86_400_000 + k * 60_000;
      out.push(testMessage({ id: `${d}-${k}`, ts, sender: "Alice", body: `msg ${d}-${k}` }));
    }
  }
  return out;
}

test("message body has no spurious leading/trailing whitespace", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({
      json: {
        messages: [testMessage({ id: "1", ts: Date.UTC(2026, 0, 1, 12), sender: "Alice", body: "Hello world" })],
        has_more: false,
        next_cursor: null,
        prev_cursor: null,
      },
    }),
  );
  await page.goto("/conversation/signal/dm:a");
  const body = page.locator(".msg .body").first();
  await body.waitFor();
  // pre-wrap shows any template whitespace as an indent.
  expect(await body.textContent()).toBe("Hello world");
});

test("favicon is linked and served", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");
  await expect(page.locator('link[rel="icon"]')).toHaveAttribute("href", "icon.svg");
  const resp = await page.request.get("/icon.svg");
  expect(resp.status()).toBe(200);
  expect(resp.headers()["content-type"]).toContain("svg");
});

test("message bubbles are not content-visibility:auto (would jump on scroll-up)", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: multiDayThread(), has_more: false, next_cursor: null, prev_cursor: null } }),
  );
  await page.goto("/conversation/signal/dm:a");
  await page.locator(".msg").first().waitFor();
  // No content-visibility: a guessed row height resized on scroll would shift
  // the viewport.
  const cv = await page.locator(".msg").first().evaluate((e) => getComputedStyle(e).contentVisibility);
  expect(cv).not.toBe("auto");
});

test("a scrolled multi-day thread does not stack date separators", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: multiDayThread(), has_more: false, next_cursor: null, prev_cursor: null } }),
  );
  await page.goto("/conversation/signal/dm:a");
  await page.getByText("msg 1-24", { exact: true }).waitFor();
  // At the bottom, the last day's header floats over its messages.
  await scrollThread(page, "bottom");
  await page.waitForTimeout(150);
  await expectNoTextOverlaps(page, testInfo);
});

test("the current day's date stays pinned at the top while scrolling", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: multiDayThread(), has_more: false, next_cursor: null, prev_cursor: null } }),
  );
  await page.goto("/conversation/signal/dm:a");
  await page.getByText("msg 0-0", { exact: true }).waitFor();
  await scrollThread(page, 400);
  await page.waitForTimeout(150);
  // Day 1's header is pinned just below the conversation head.
  const threadTop = await page.locator(".thread").evaluate((e) => e.getBoundingClientRect().top);
  // en-GB `fullDate`, from LOCALE_ID in app.config.ts; Angular defaults to en-US.
  const box = await page.getByText("Thursday, 1 January 2026", { exact: true }).boundingBox();
  expect(box).not.toBeNull();
  const offset = (box?.y ?? -999) - threadTop;
  expect(offset).toBeGreaterThanOrEqual(40); // pinned below the sticky head, not scrolled off
  expect(offset).toBeLessThan(90); // pinned, not at its in-flow position far down
});
