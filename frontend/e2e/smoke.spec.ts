import { expect, test, type Page } from "@playwright/test";
import { expectIconFontLoaded, expectNoTextOverlaps } from "@xinutec/ui-harness";

/**
 * Render the authenticated app shell at a phone viewport with the backend
 * mocked, and assert the render is sound: the Material Icons font loaded (so the
 * search/back/attachment glyphs aren't their ligature words) and no text
 * collides. This is the check that would have caught the icon-font regression —
 * unit tests (jsdom) can't see fonts or layout.
 */

const ME = { user_id: "u1", display_name: "Test User" };

const CONVERSATIONS = [
  { origin: "signal", id: "dm:a", name: "Alice", kind: "dm", message_count: 5, last_ts: 1_717_000_000_000 },
  { origin: "gchat", id: "gc1", name: "Bob", kind: "dm", message_count: 3, last_ts: 1_717_100_000_000 },
];

/** Mock every backend call the shell makes so it renders with no server/auth.
 *  Catch-all registered FIRST: Playwright runs handlers last-registered-first. */
async function mockApi(page: Page): Promise<void> {
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
}

test("authenticated shell renders: icon font loaded, no text overlaps @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/");
  // The search field (with its prefix icon) is the spot the bug showed up.
  await page.getByPlaceholder("Search messages").waitFor();
  await page.getByText("Alice").waitFor();
  await expectIconFontLoaded(page);
  await expectNoTextOverlaps(page, testInfo);
});

// Two days, each tall enough (25 messages) to exceed the viewport so a day's
// sticky header actually pins while scrolling within it.
function multiDayThread() {
  const base = Date.UTC(2026, 0, 1, 12, 0, 0); // Jan 1 (Thu), Jan 2 (Fri)
  const out = [];
  for (let d = 0; d < 2; d++) {
    for (let k = 0; k < 25; k++) {
      const ts = base + d * 86_400_000 + k * 60_000;
      out.push({ id: `${d}-${k}`, ts, sender: "Alice", is_outgoing: false, body: `msg ${d}-${k}`, deleted: false, edited: false, reactions: [], attachments: [] });
    }
  }
  return out;
}

test("message body has no spurious leading/trailing whitespace", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({
      json: {
        messages: [{ id: "1", ts: Date.UTC(2026, 0, 1, 12), sender: "Alice", is_outgoing: false, body: "Hello world", deleted: false, edited: false, reactions: [], attachments: [] }],
        has_more: false,
        next_cursor: null,
      },
    }),
  );
  await page.goto("/conversation/signal/dm:a");
  const body = page.locator(".msg .body").first();
  await body.waitFor();
  // pre-wrap preserves whitespace, so any template-introduced leading space
  // would show as a first-line indent. The rendered text must equal the body.
  expect(await body.textContent()).toBe("Hello world");
});

test("favicon is linked and served", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");
  await expect(page.locator('link[rel="icon"]')).toHaveAttribute("href", "icon.svg");
  // Served from public/ via the assets glob (same wiring as the other apps).
  const resp = await page.request.get("/icon.svg");
  expect(resp.status()).toBe(200);
  expect(resp.headers()["content-type"]).toContain("svg");
});

test("message bubbles are not content-visibility:auto (would jump on scroll-up)", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: multiDayThread(), has_more: false, next_cursor: null } }),
  );
  await page.goto("/conversation/signal/dm:a");
  await page.locator(".msg").first().waitFor();
  // The rendered window is capped in thread.ts, so bubbles render in full.
  // content-visibility:auto would render off-screen rows at a guessed height and
  // resize them when scrolled into view — shifting the viewport (the reported
  // "history jumps as you scroll up" bug).
  const cv = await page.locator(".msg").first().evaluate((e) => getComputedStyle(e).contentVisibility);
  expect(cv).not.toBe("auto");
});

test("a scrolled multi-day thread does not stack date separators", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: multiDayThread(), has_more: false, next_cursor: null } }),
  );
  await page.goto("/conversation/signal/dm:a");
  await page.getByText("msg 1-24", { exact: true }).waitFor();
  // Scroll the thread to the bottom — where the sticky bug piled the dates up,
  // and where the last day's header now floats (sticky) over its messages.
  await page.evaluate(() => {
    const t = document.querySelector(".thread");
    if (t) t.scrollTop = t.scrollHeight;
  });
  await page.waitForTimeout(150);
  await expectNoTextOverlaps(page, testInfo);
});

test("the current day's date stays pinned at the top while scrolling", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: multiDayThread(), has_more: false, next_cursor: null } }),
  );
  await page.goto("/conversation/signal/dm:a");
  await page.getByText("msg 0-0", { exact: true }).waitFor();
  // Scroll down within the first (tall) day so its header has scrolled past.
  await page.evaluate(() => {
    const t = document.querySelector(".thread");
    if (t) t.scrollTop = 400;
  });
  await page.waitForTimeout(150);
  // Day 1's header is pinned just below the sticky conversation head (~3.25rem),
  // not scrolled away above it (large negative offset) nor sitting at its far-down
  // in-flow position.
  const threadTop = await page.locator(".thread").evaluate((e) => e.getBoundingClientRect().top);
  const box = await page.getByText("Thursday, January 1, 2026", { exact: true }).boundingBox();
  expect(box).not.toBeNull();
  const offset = (box?.y ?? -999) - threadTop;
  expect(offset).toBeGreaterThanOrEqual(40); // pinned below the sticky head, not scrolled off
  expect(offset).toBeLessThan(90); // pinned, not at its in-flow position far down
});
