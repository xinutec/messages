import { expect, test, type Page } from "@playwright/test";

/**
 * Navigation state lives in the URL: `/conversation/:origin/:id`, with
 * `?origin` and `?from` as query params, so it survives refresh and Back works.
 */

const ME = { user_id: "u1", display_name: "Test User" };
const CONVERSATIONS = [
  { origin: "signal", id: "dm:a", name: "Alice", kind: "dm", message_count: 5, last_ts: 1_717_000_000_000 },
  { origin: "gchat", id: "gc1", name: "Bob", kind: "dm", message_count: 3, last_ts: 1_717_100_000_000 },
];
const MESSAGES_PAGE = {
  messages: [
    { id: "1", ts: 1_717_000_000_000, sender: "Alice", is_outgoing: false, body: "hi", deleted: false, edited: false, reactions: [], attachments: [], link_images: [], link_offers: [], edits: [] },
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
  // exact: a gchat row's subtitle also contains "Google Chat".
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
  await expect(page.getByText("Alice")).toHaveCount(0);
});

function m(ts: number, body: string) {
  return { id: String(ts), ts, sender: "s", is_outgoing: false, body, deleted: false, edited: false, reactions: [], attachments: [], link_images: [], link_offers: [], edits: [] };
}

function bulk(prefix: string, startTs: number, n: number) {
  return Array.from({ length: n }, (_, k) => m(startTs + k * 10, `${prefix}${k}`));
}

/** Scroll the thread to the top, failing if there is no `.thread` rather than
 *  silently doing nothing. */
const scrollThreadTop = async (page: Page) => {
  await page.locator(".thread").waitFor();
  const scrolled = await page.evaluate(() => {
    const t = document.querySelector(".thread");
    if (!t) return null;
    t.scrollTop = 0;
    return t.scrollTop;
  });
  if (scrolled === null) {
    throw new Error("scrollThreadTop: .thread is not in the DOM — nothing was scrolled");
  }
};

// A tall recent page with an older page below it: a cursor gets the older.
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
  // Older pages load on scroll-up.
  await expect(page.getByRole("button", { name: /Load older/ })).toHaveCount(0);
  await scrollThreadTop(page);
  // In the DOM, above the viewport.
  await page.getByText("antique0", { exact: true }).waitFor({ state: "attached" });
});

test("scroll position is reflected in ?from", async ({ page }) => {
  await mockApi(page);
  // One tall page (oldest ts 1000), nothing older.
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: bulk("only", 1000, 40), has_more: false, next_cursor: null, prev_cursor: null } }),
  );
  await page.goto("/conversation/signal/dm:a");
  await page.getByText("only39", { exact: true }).waitFor();
  await scrollThreadTop(page);
  // The top message's ts is written to `?from`, debounced.
  await page.waitForURL(/[?&]from=1000\b/);
});

/** The `?from` debounce must not fire after leaving the conversation: it
 *  navigates with `replaceUrl`, which would put the reader back in it. */
test("leaving during the ?from debounce does not navigate back into the thread", async ({ page }) => {
  await mockApi(page);
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: bulk("only", 1000, 40), has_more: false, next_cursor: null, prev_cursor: null } }),
  );
  await page.goto("/");

  await page.getByRole("button", { name: /Alice/ }).click();
  await page.getByText("only39", { exact: true }).waitFor();
  await scrollThreadTop(page);
  // 150ms: past the 60ms re-check, before the 300ms debounce, so the timer is
  // armed and unfired when we leave.
  await page.waitForTimeout(150);
  await page.goBack();
  await expect(page).not.toHaveURL(/\/conversation\//);
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
 * A visibility event refetches the list and the changed count reaches the
 * screen.
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

  // As Chrome does on resume: already visible, and this event the only signal.
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));

  await expect(page.getByText(/9 msgs/)).toBeVisible();
});
