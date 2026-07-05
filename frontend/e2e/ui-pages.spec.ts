import { test, type Page } from "@playwright/test";
// The fleet-shared harness, published as @xinutec/ui-harness (source repo
// ~/Code/ui-harness). Ships compiled JS, so it loads straight from node_modules.
import {
  expectNoTextOverlaps,
  expectNoHorizontalOverflow,
  expectViewportIsPhone,
  expectIconFontLoaded,
} from "@xinutec/ui-harness";

/**
 * L2 phone-width layout harness for messages. Render the two real screens (the
 * conversation-list shell and an open thread) at a Pixel viewport with the
 * backend mocked and BUSY data, and assert the two failure classes that read
 * fine in source and only show on a real phone:
 *   1. no two pieces of rendered text collide, and
 *   2. nothing spills past the right edge.
 * The at-risk spots here: the three-button origin filter row (All / Signal /
 * Google Chat) crowding at 412px, and a message's meta line (sender + time +
 * "edited") and reaction chips overflowing or overlapping the body.
 *
 * There is no service worker in this app, but block it anyway for parity with
 * the fleet's layout specs — SW-controlled fetches would bypass page.route.
 */
test.use({ serviceWorkers: "block" });

const ME = { user_id: "test", display_name: "Test User" };

/** A busy conversation list: both origins, a group, a deliberately long name to
 *  stress the row title's ellipsis, and a long-tail of counts/dates. */
const CONVERSATIONS = [
  { origin: "signal", id: "dm:a", name: "Alice Andersson", kind: "dm", message_count: 128, last_ts: Date.UTC(2026, 0, 2, 9, 14) },
  { origin: "signal", id: "grp:x", name: "Saturday climbing & bouldering logistics crew", kind: "group", message_count: 4210, last_ts: Date.UTC(2026, 0, 1, 20, 2) },
  { origin: "gchat", id: "gc1", name: "Bob Bytecode", kind: "dm", message_count: 37, last_ts: Date.UTC(2025, 11, 30, 16, 40) },
  { origin: "gchat", id: "gc2", name: "Platform on-call", kind: "group", message_count: 902, last_ts: Date.UTC(2025, 11, 29, 8, 5) },
];

/** A busy thread: long sender, a long unbroken-ish body, an "edited" tag, an
 *  unavailable attachment, and a row of reaction chips — every element that can
 *  crowd or overflow the bubble. */
const THREAD = {
  messages: [
    { id: "1", ts: Date.UTC(2026, 0, 1, 12, 0), sender: "Alice Andersson", is_outgoing: false,
      body: "Morning! Did the referral letter come through yet? The clinic said they'd post it but it's been almost two weeks now.",
      deleted: false, edited: true, reactions: [{ emoji: "👍", count: 3 }, { emoji: "❤️", count: 2 }, { emoji: "🎉", count: 1 }],
      attachments: [] },
    { id: "2", ts: Date.UTC(2026, 0, 1, 12, 4), sender: "Test User", is_outgoing: true,
      body: "Not yet — chasing them this afternoon.", deleted: false, edited: false, reactions: [],
      attachments: [{ id: "a1", content_type: "application/pdf", file_name: "referral-scan-2026-final-v2.pdf", size: 91234, available: false, is_image: false }] },
    { id: "3", ts: Date.UTC(2026, 0, 1, 12, 9), sender: "Alice Andersson", is_outgoing: false,
      body: "Thankyouuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuu", deleted: false, edited: false, reactions: [], attachments: [] },
  ],
  has_more: false,
  next_cursor: null,
};

/** Mock every backend call: signed in, the busy list, the busy thread.
 *  Catch-all FIRST — Playwright runs handlers last-registered-first. */
async function mockApi(page: Page): Promise<void> {
  await page.route("**/api/**", (r) =>
    r.request().method() === "GET" ? r.fulfill({ json: [] }) : r.fulfill({ status: 204, body: "" }),
  );
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
  await page.route("**/api/conversations/**/messages**", (r) => r.fulfill({ json: THREAD }));
}

// The checker-checker: fail loudly here if the device preset is ever lost and
// the "phone width" suite silently runs at desktop width (defect 2).
test("the suite really runs at phone geometry", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");
  await expectViewportIsPhone(page);
});

test("conversation list — filter row + rows: lays out cleanly @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/");
  await page.getByPlaceholder("Search messages").waitFor();
  await page.getByRole("button", { name: "Google Chat", exact: true }).waitFor(); // widest filter button
  await page.getByText("Alice Andersson").waitFor();
  // The search field's prefix icon is where an icon-font fallback shows up as
  // the literal word "search" overlapping the placeholder (guarded here since
  // the shell is the screen with mat-icons).
  await expectIconFontLoaded(page);
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});

test("open thread — meta + reactions + attachment: lays out cleanly @ phone width", async ({ page }, testInfo) => {
  await mockApi(page);
  await page.goto("/conversation/signal/dm:a");
  // Wait for the far messages so the whole thread has laid out before measuring.
  await page.locator(".msg .body").first().waitFor();
  await page.getByText("👍 3").waitFor();
  await page.getByText("referral-scan-2026-final-v2.pdf", { exact: false }).waitFor();
  await expectNoTextOverlaps(page, testInfo);
  await expectNoHorizontalOverflow(page, testInfo);
});
