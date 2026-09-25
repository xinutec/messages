import { expect, test, type Page } from "@playwright/test";

import type { MessageKind } from "../src/app/models";
import { testMessage } from "../src/app/test-message";

/**
 * Copying a selection out of a thread with a real drag, ⌘C and clipboard;
 * jsdom's Selection API is wrong here.
 */

// Fixed, so the rendered clock does not follow the machine's time zone.
test.use({ timezoneId: "UTC", permissions: ["clipboard-read", "clipboard-write"] });

const ME = { user_id: "u1", display_name: "Test User" };
/** Matches the four messages served, or every copy would report truncation. */
const conversations = (total = 4) => [
  { origin: "irc", id: "7", name: "#chan", kind: "group", network: "xinutec", message_count: total, last_ts: Date.UTC(2026, 7, 14, 9, 5) },
];
const line = (id: string, ts: number, sender: string, body: string, kind: MessageKind = "message") =>
  testMessage({ id, ts, sender, body, kind });

const MESSAGES = [
  line("a", Date.UTC(2026, 7, 13, 14, 32), "pippijn", "hello there"),
  line("b", Date.UTC(2026, 7, 13, 14, 33), "simon", "hi"),
  line("d", Date.UTC(2026, 7, 13, 14, 34), "pippijn", "waves", "action"),
  line("c", Date.UTC(2026, 7, 14, 9, 5), "simon", "morning"),
];

/** The catch-all goes first: Playwright runs handlers last-registered-first. */
async function openThread(page: Page, total = 4): Promise<void> {
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: conversations(total) }));
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: MESSAGES, has_more: false, next_cursor: null, prev_cursor: null } }),
  );
  await page.goto("/conversation/irc/7");
  await page.locator('.msg[data-id="c"] .body').waitFor();
}

/** Drag across two message bodies, from just inside the first to just inside
 *  the last. */
async function dragSelect(page: Page, from: string, to: string): Promise<void> {
  const a = (await page.locator(`.msg[data-id="${from}"] .body`).boundingBox())!;
  const b = (await page.locator(`.msg[data-id="${to}"] .body`).boundingBox())!;
  await page.mouse.move(a.x + 2, a.y + a.height / 2);
  await page.mouse.down();
  await page.mouse.move(b.x + b.width - 2, b.y + b.height / 2, { steps: 12 });
  await page.mouse.up();
}

const clipboardText = (page: Page) => page.evaluate(() => navigator.clipboard.readText());

test("a selection spanning messages copies as an irssi log", async ({ page }) => {
  await openThread(page);
  await dragSelect(page, "a", "c");
  await page.keyboard.press("ControlOrMeta+c");
  expect(await clipboardText(page)).toBe(
    "--- Day changed Thu Aug 13 2026\n" +
      "14:32 <pippijn> hello there\n" +
      "14:33 <simon> hi\n" +
      // irssi's action line: two spaces, the sender inside the text.
      "14:34  * pippijn waves\n" +
      "--- Day changed Fri Aug 14 2026\n" +
      "09:05 <simon> morning",
  );
});

test("a selection inside one message copies the words, and nothing else", async ({ page }) => {
  await openThread(page);
  await dragSelect(page, "b", "b");
  await page.keyboard.press("ControlOrMeta+c");
  expect(await clipboardText(page)).toBe("hi");
});

test("dragging over a whole bubble copies the sentence, not the nick and clock", async ({ page }) => {
  await openThread(page);
  // Checks `user-select: none`, so the drag starts on the meta row, at the
  // bubble's top corner.
  const bubble = (await page.locator('.msg[data-id="b"]').boundingBox())!;
  const body = (await page.locator('.msg[data-id="b"] .body').boundingBox())!;
  await page.mouse.move(bubble.x + 2, bubble.y + 2);
  await page.mouse.down();
  await page.mouse.move(body.x + body.width - 2, body.y + body.height / 2, { steps: 12 });
  await page.mouse.up();
  await page.keyboard.press("ControlOrMeta+c");
  expect(await clipboardText(page)).toBe("hi");
});


test("the rich flavour is the same log, monospaced", async ({ page }) => {
  await openThread(page);
  await dragSelect(page, "a", "b");
  await page.keyboard.press("ControlOrMeta+c");
  const html = await page.evaluate(async () => {
    const [item] = await navigator.clipboard.read();
    return item.types.includes("text/html")
      ? await (await item.getType("text/html")).text()
      : "(no text/html on the clipboard)";
  });
  expect(html).toContain("<pre>");
  expect(html).toContain("&lt;pippijn&gt;");
});

/** A select-all copies only the rendered window and says so. Here the window
 *  is all four messages and the conversation claims far more. */
test("a select-all that only got the window says so in the paste", async ({ page }) => {
  await openThread(page, 401794);
  await dragSelect(page, "a", "c");
  await page.keyboard.press("ControlOrMeta+c");
  const text = await clipboardText(page);
  expect(text).toContain("--- copied 4 of 401794 messages; the rest were not loaded on screen");
  expect(text.split("\n").at(-1)).toMatch(/^--- copied 4 of 401794 messages/);
});

test("a quote of two messages carries no such notice", async ({ page }) => {
  await openThread(page, 401794);
  await dragSelect(page, "a", "b");
  await page.keyboard.press("ControlOrMeta+c");
  const text = await clipboardText(page);
  expect(text).toContain("hello there");
  expect(text).not.toContain("not loaded on screen");
});