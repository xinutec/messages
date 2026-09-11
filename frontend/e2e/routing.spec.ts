import { expect, test, type Page } from "@playwright/test";

/**
 * The router contract: navigation state lives in the URL, so it's bookmarkable/
 * shareable, survives refresh, and the browser Back button works. The open
 * conversation is a real route — `/conversation/:origin/:id` — and the origin filter and
 * paged-back depth are query params (`?origin` / `?from`). Behavioural, because
 * "is the router used correctly?" is a render/navigation fact a static rule
 * can't see.
 */

const ME = { user_id: "u1", display_name: "Test User" };
const CONVERSATIONS = [
  { origin: "signal", id: "dm:a", name: "Alice", kind: "dm", message_count: 5, last_ts: 1_717_000_000_000 },
  { origin: "gchat", id: "gc1", name: "Bob", kind: "dm", message_count: 3, last_ts: 1_717_100_000_000 },
];
const MESSAGES_PAGE = {
  messages: [
    { id: "1", ts: 1_717_000_000_000, sender: "Alice", is_outgoing: false, body: "hi", deleted: false, edited: false, reactions: [], attachments: [] },
  ],
  has_more: false,
  next_cursor: null,
};

async function mockApi(page: Page): Promise<void> {
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
  await page.route("**/api/conversations/**/messages**", (r) => r.fulfill({ json: MESSAGES_PAGE }));
}

test("origin filter is reflected in the URL", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");
  // exact: the filter button's name is exactly "Google Chat"; a gchat
  // conversation row also contains "Google Chat" in its subtitle.
  await page.getByRole("button", { name: "Google Chat", exact: true }).click();
  await expect(page).toHaveURL(/[?&]origin=gchat\b/);
});

test("opening a conversation is reflected in the URL", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");
  await page.getByRole("button", { name: /Alice/ }).click();
  await expect(page).toHaveURL(/\/conversation\/signal\/dm:a/);
});

test("deep-linking an origin filter restores it on load", async ({ page }) => {
  await mockApi(page);
  await page.goto("/?origin=gchat");
  await page.getByText("Bob").waitFor();
  // Signal conversations are filtered out, so Alice must not be listed.
  await expect(page.getByText("Alice")).toHaveCount(0);
});

function m(ts: number, body: string) {
  return { id: String(ts), ts, sender: "s", is_outgoing: false, body, deleted: false, edited: false, reactions: [], attachments: [] };
}

function bulk(prefix: string, startTs: number, n: number) {
  return Array.from({ length: n }, (_, k) => m(startTs + k * 10, `${prefix}${k}`));
}

const scrollThreadTop = (page: Page) =>
  page.evaluate(() => {
    const t = document.querySelector(".thread");
    if (t) t.scrollTop = 0;
  });

// Paged mock: a tall recent page (enough rows to scroll) that has an older page
// below it. `cursor` present → the older page; absent → the fresh page.
async function mockApiPaged(page: Page): Promise<void> {
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
  await page.route("**/api/conversations/**/messages**", async (route) => {
    const cursor = new URL(route.request().url()).searchParams.get("cursor");
    if (cursor) {
      await route.fulfill({
        json: { messages: bulk("antique", 1000, 30), has_more: false, next_cursor: null, prev_cursor: null },
      });
    } else {
      await route.fulfill({
        json: { messages: bulk("fresh", 5000, 30), has_more: true, next_cursor: "5000", prev_cursor: null },
      });
    }
  });
}

test("scrolling to the top auto-loads older messages (no button)", async ({ page }) => {
  await mockApiPaged(page);
  await page.goto("/conversation/signal/dm:a");
  await page.getByText("fresh29", { exact: true }).waitFor();
  // Chat-style: the manual "Load older" button is gone; older pages load on
  // scroll-up.
  await expect(page.getByRole("button", { name: /Load older/ })).toHaveCount(0);
  await scrollThreadTop(page);
  // The older page is fetched and rendered (it lands above the viewport, so
  // assert it's in the DOM rather than in view).
  await page.getByText("antique0", { exact: true }).waitFor({ state: "attached" });
});

test("scroll position is reflected in ?from", async ({ page }) => {
  await mockApi(page);
  // One tall page (oldest ts = 1000), no older pages.
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: bulk("only", 1000, 40), has_more: false, next_cursor: null, prev_cursor: null } }),
  );
  await page.goto("/conversation/signal/dm:a");
  await page.getByText("only39", { exact: true }).waitFor();
  await scrollThreadTop(page);
  // The message at the top of the viewport (ts 1000) is written to ?from
  // (debounced), so a refresh returns here.
  await page.waitForURL(/[?&]from=1000\b/);
});

/** ⚠ **A DEBOUNCED NAVIGATION OUTLIVES THE SCREEN THAT ARMED IT.** `?from` is
 *  written 300ms after a scroll; leave inside that window and the timer fired on
 *  a destroyed component, navigating relative to a route the reader had already
 *  left — with `replaceUrl: true`, so Back appeared to work and then put them
 *  back in the conversation, the list entry gone from history. Silent: no page
 *  error, nothing logged.
 *
 *  ⚠ **THE 150ms IS THE TEST.** Scrolling and leaving immediately is a race —
 *  whether the timer is still pending when Back lands — and two drafts died on
 *  it. One asserted the URL mid-flight to prove the debounce was pending; that
 *  assertion costs a round trip, the timer fired inside it, and the case passed
 *  against the defect 3 times in 3. A second retried in a loop and still only
 *  caught it 1 time in 4. Waiting a fixed 150ms instead — past the 60ms
 *  re-check, short of the 300ms debounce — puts the timer reliably ARMED AND
 *  UNFIRED at the moment of leaving: ablate the teardown and this fails 4 of 4,
 *  restore it and it passes 4 of 4. */
test("leaving during the ?from debounce does not navigate back into the thread", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: bulk("only", 1000, 40), has_more: false, next_cursor: null, prev_cursor: null } }),
  );
  await page.goto("/");

  await page.getByRole("button", { name: /Alice/ }).click();
  await page.getByText("only39", { exact: true }).waitFor();
  await scrollThreadTop(page);
  // ⚠ 150ms: ARMED BUT NOT YET FIRED, which is what makes this deterministic
  // rather than a race. The debounce is 300ms and a scroll skipped while the
  // window is busy re-checks after 60, so by 150 the timer exists and has not
  // run. Leaving now is the case under test.
  await page.waitForTimeout(150);
  await page.goBack();
  await expect(page).not.toHaveURL(/\/conversation\//);
  // Its moment passes without moving us.
  await page.waitForTimeout(600);
  await expect(page).not.toHaveURL(/\/conversation\//);
});

test("reloading restores the older messages that were paged in", async ({ page }) => {
  await mockApiPaged(page);
  await page.goto("/conversation/signal/dm:a?from=1000"); // as if reloaded after scrolling back
  await page.getByText("antique0", { exact: true }).waitFor({ state: "attached" }); // restored, no scroll
  await page.getByText("fresh0", { exact: true }).waitFor({ state: "attached" });
});

test("Back returns from a conversation to the list", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");
  await page.getByRole("button", { name: /Alice/ }).click();
  await expect(page).toHaveURL(/\/conversation\/signal\/dm:a/);
  await page.goBack();
  await expect(page).not.toHaveURL(/\/conversation\//);
  await page.getByPlaceholder("Search messages").waitFor();
});

/**
 * The whole chain the unit test cannot reach: the visibility event, through the
 * store's refetch, to a changed number on screen. The mock answers with a higher
 * count the second time, which is what an idle two hours looks like.
 */
test("returning to the foreground re-reads the list", async ({ page }) => {
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  let calls = 0;
  await page.route("**/api/conversations", (r) =>
    r.fulfill({ json: [{ ...CONVERSATIONS[0], message_count: ++calls === 1 ? 5 : 9 }] }),
  );

  await page.goto("/");
  await expect(page.getByText(/5 msgs/)).toBeVisible();

  // What Chrome does when the app is brought back: the document is already
  // 'visible', and this event is the only announcement of it.
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));

  await expect(page.getByText(/9 msgs/)).toBeVisible();
});
