import { expect, test, type Page } from "@playwright/test";
import { expectIconFontLoaded, expectNoTextOverlaps } from "@xinutec/ui-harness";

/**
 * Render the authenticated app shell at a phone viewport with the backend
 * mocked, and assert the render is sound: the Material Icons font loaded (so the
 * search/back/attachment glyphs aren't their ligature words) and no text
 * collides. This is the check that would have caught the icon-font regression —
 * unit tests (jsdom) can't see fonts or layout.
 */

/** Scroll `.thread` to `top` — a pixel offset, or "bottom" — and refuse to
 *  pretend when there is nothing to scroll.
 *
 *  ⚠ **Both callers assert about what is on screen AFTER scrolling**, so a
 *  silent no-op does not merely fail — it can PASS, against an unscrolled page.
 *  That is the quieter half of the fault that held #1243 and the messages
 *  routing flake open for weeks: `if (t)` turning "container absent" into
 *  "scrolled fine". */
async function scrollThread(page: Page, top: number | "bottom"): Promise<void> {
  await page.locator(".thread").waitFor();
  // ⚠ SETTING scrollTop ONCE IS NOT SCROLLING — THE COMPONENT SCROLLS ITSELF
  // AND CAN DO IT AFTER US. Opening a thread jumps to the newest message. This
  // ran as soon as `.thread` existed, and on a loaded machine that jump landed
  // SECOND: the gate caught it on 2026-09-21 (three gates, load 10.2 on 10 cores)
  // as a day header 635.6875px above where it belonged. That number is not noise
  // — measured against this fixture, it is the offset at scrollTop 2042, the
  // BOTTOM. The page was never at 400.
  //
  // So the position is re-applied until it survives three consecutive frames.
  // Re-applying is not papering over the jump: a reader scrolling up after the
  // thread settles does exactly this. An app that kept yanking would run the
  // frame budget out and leave the caller's assertion to fail, which is the
  // outcome worth having.
  const settled = await page.evaluate(async (to) => {
    const t = document.querySelector(".thread");
    if (!t) return null;
    // "bottom" is scrollHeight MINUS the visible height; scrollTop can never
    // reach scrollHeight, so comparing against it would never hold.
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
      out.push({ id: `${d}-${k}`, ts, sender: "Alice", is_outgoing: false, body: `msg ${d}-${k}`, deleted: false, edited: false, reactions: [], attachments: [], link_images: [], link_offers: [], edits: [] });
    }
  }
  return out;
}

test("message body has no spurious leading/trailing whitespace", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({
      json: {
        messages: [{ id: "1", ts: Date.UTC(2026, 0, 1, 12), sender: "Alice", is_outgoing: false, body: "Hello world", deleted: false, edited: false, reactions: [], attachments: [], link_images: [], link_offers: [], edits: [] }],
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
    r.fulfill({ json: { messages: multiDayThread(), has_more: false, next_cursor: null, prev_cursor: null } }),
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
    r.fulfill({ json: { messages: multiDayThread(), has_more: false, next_cursor: null, prev_cursor: null } }),
  );
  await page.goto("/conversation/signal/dm:a");
  await page.getByText("msg 1-24", { exact: true }).waitFor();
  // Scroll the thread to the bottom — where the sticky bug piled the dates up,
  // and where the last day's header now floats (sticky) over its messages.
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
  // Scroll down within the first (tall) day so its header has scrolled past.
  await scrollThread(page, 400);
  await page.waitForTimeout(150);
  // Day 1's header is pinned just below the sticky conversation head (~3.25rem),
  // not scrolled away above it (large negative offset) nor sitting at its far-down
  // in-flow position.
  const threadTop = await page.locator(".thread").evaluate((e) => e.getBoundingClientRect().top);
  // ⚠ The en-GB rendering of `fullDate` — day before month. It said "Thursday,
  // January 1, 2026" until app.config.ts provided LOCALE_ID, because Angular
  // defaults it to en-US whatever the browser says. If this string ever needs
  // changing again, check the provider before changing the test: an assertion on
  // a rendered date is an assertion about the app's locale.
  const box = await page.getByText("Thursday, 1 January 2026", { exact: true }).boundingBox();
  expect(box).not.toBeNull();
  const offset = (box?.y ?? -999) - threadTop;
  expect(offset).toBeGreaterThanOrEqual(40); // pinned below the sticky head, not scrolled off
  expect(offset).toBeLessThan(90); // pinned, not at its in-flow position far down
});
