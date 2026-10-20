import { expect, test, type Page, type Route } from "@playwright/test";
import { expectIconFontLoaded, expectCleanLayout } from "@xinutec/ui-harness";

import type { Conversation, Me, Message, MessagesPage } from "../src/app/models";
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
        // The reader's hand first: only a gesture's scroll is theirs (thread.ts).
        t.dispatchEvent(new WheelEvent("wheel", { bubbles: true }));
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

const ME = { user_id: "u1", display_name: "Test User" } satisfies Me;

const CONVERSATIONS = [
  { origin: "signal", id: "dm:a", name: "Alice", kind: "dm", network: null, message_count: 5, last_ts: 1_717_000_000_000, last: null, unread: 0, avatar: null },
  { origin: "gchat", id: "gc1", name: "Bob", kind: "dm", network: null, message_count: 3, last_ts: 1_717_100_000_000, last: null, unread: 0, avatar: null },
] satisfies Conversation[];

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
  await expectCleanLayout(page, testInfo);
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
      } satisfies MessagesPage,
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
    r.fulfill({ json: { messages: multiDayThread(), has_more: false, next_cursor: null, prev_cursor: null } satisfies MessagesPage }),
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
    r.fulfill({ json: { messages: multiDayThread(), has_more: false, next_cursor: null, prev_cursor: null } satisfies MessagesPage }),
  );
  await page.goto("/conversation/signal/dm:a");
  await page.getByText("msg 1-24", { exact: true }).waitFor();
  // At the bottom, the last day's header floats over its messages.
  await scrollThread(page, "bottom");
  await page.waitForTimeout(150);
  await expectCleanLayout(page, testInfo);
});

test("the day on view floats at the top while scrolling, and fades once the reader stops", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: multiDayThread(), has_more: false, next_cursor: null, prev_cursor: null } satisfies MessagesPage }),
  );
  await page.goto("/conversation/signal/dm:a");
  await page.getByText("msg 0-0", { exact: true }).waitFor();
  await scrollThread(page, 400);
  await page.waitForTimeout(150);
  // Day 1's pill floats at the top of the thread, under the bar; its in-flow
  // separator has scrolled off above.
  const pill = page.locator(".day.floating");
  await expect(pill).toHaveClass(/shown/);
  // en-GB `fullDate`, from LOCALE_ID in app.config.ts; Angular defaults to en-US.
  await expect(pill).toHaveText("Thursday, 1 January 2026");
  const threadTop = await page.locator(".thread").evaluate((e) => e.getBoundingClientRect().top);
  const box = await pill.locator("span").boundingBox();
  const offset = (box?.y ?? -999) - threadTop;
  expect(offset).toBeGreaterThanOrEqual(0);
  expect(offset).toBeLessThan(40);
  // Signal's pill goes once the reader stops; the separators stay in the flow.
  await page.waitForTimeout(1600);
  await expect(pill).not.toHaveClass(/shown/);
});

/** What the worker kept shows before the network answers (messages-store.ts,
 *  thread.ts): a list and a page seeded into the Cache API as ngsw stores them
 *  (the readers match by URL across every cache), and a network held back three
 *  seconds. Seen on the phone first: a bar over nothing for the whole round trip. */
const pageOf = (messages: Message[]) => ({ messages, has_more: false, next_cursor: null, prev_cursor: null }) satisfies MessagesPage;

/** Open the list once, signed in, then seed the Cache API as ngsw stores the
 *  list and dm:a's newest page (the readers match by URL across every cache). */
async function withKept(page: Page): Promise<Message[]> {
  await mockApi(page);
  await page.goto("/");
  await page.getByText("Alice").waitFor();
  const kept = multiDayThread().slice(0, 3);
  await page.evaluate(
    async ({ list, thread }) => {
      const c = await caches.open("seed");
      const json = (v: unknown) => new Response(JSON.stringify(v), { headers: { "content-type": "application/json" } });
      await c.put(new Request("/api/conversations"), json(list));
      await c.put(new Request("/api/conversations/signal/dm%3Aa/messages?limit=100"), json(thread));
    },
    { list: CONVERSATIONS, thread: pageOf(kept) },
  );
  return kept;
}

test("a kept list and thread are on screen at once while the network is slow", async ({ page }) => {
  const kept = await withKept(page);
  const slow = async (r: Route, json: Conversation[] | MessagesPage) => {
    await new Promise((res) => setTimeout(res, 3000));
    await r.fulfill({ json });
  };
  await page.route("**/api/conversations", (r) => slow(r, CONVERSATIONS));
  await page.route("**/api/conversations/**/messages**", (r) => slow(r, pageOf(kept)));

  let t0 = Date.now();
  await page.goto("/conversation/signal/dm:a");
  await page.locator(`.msg[data-id="${kept[0].id}"]`).waitFor();
  expect(Date.now() - t0).toBeLessThan(2000);
  await expect(page.locator(".loading-thread")).toBeVisible();

  t0 = Date.now();
  await page.goto("/");
  await page.getByText("Alice").waitFor();
  expect(Date.now() - t0).toBeLessThan(2000);
});

/** With no network at all, the kept thread stays on view, no "couldn't load",
 *  and the shell says what is on view is what was last seen. */
test("offline, a kept thread stays on view and the shell says so", async ({ page }) => {
  const kept = await withKept(page);
  await page.route("**/api/conversations", (r) => r.abort("internetdisconnected"));
  await page.route("**/api/conversations/**/messages**", (r) => r.abort("internetdisconnected"));
  await page.goto("/conversation/signal/dm:a");
  await page.locator(`.msg[data-id="${kept[0].id}"]`).waitFor();
  await expect(page.locator(".offline-note")).toHaveText(/Offline, showing what was last seen/);
  await expect(page.locator(".loading-thread")).toBeHidden();
  await expect(page.getByText("Couldn't load this conversation")).toBeHidden();
  await expect(page.locator(`.msg[data-id="${kept[0].id}"]`)).toBeVisible();
});
