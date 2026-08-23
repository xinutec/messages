import { expect, test, type Page } from "@playwright/test";

/**
 * Copying a selection out of a thread, in a real browser.
 *
 * The unit tests (jsdom) cover the same paths, and jsdom is not trustworthy
 * here: measured 2026-08-16, its `Selection.containsNode` answered true for a
 * bubble entirely past the range and false for the bubble a small selection sat
 * inside. So the DOM half of this feature gets checked where it actually runs —
 * a real drag, the real ⌘C, and the real system clipboard.
 */

// Fixed, so the rendered clock does not follow whoever is running this.
test.use({ timezoneId: "UTC", permissions: ["clipboard-read", "clipboard-write"] });

const ME = { user_id: "u1", display_name: "Test User" };
const CONVERSATIONS = [
  { origin: "irc", id: "7", name: "#chan", kind: "group", network: "xinutec", message_count: 3, last_ts: Date.UTC(2026, 7, 14, 9, 5) },
];
const MESSAGES = [
  { id: "a", ts: Date.UTC(2026, 7, 13, 14, 32), sender: "pippijn", body: "hello there", is_outgoing: false, deleted: false, edited: false, reactions: [], attachments: [] },
  { id: "b", ts: Date.UTC(2026, 7, 13, 14, 33), sender: "simon", body: "hi", is_outgoing: false, deleted: false, edited: false, reactions: [], attachments: [] },
  { id: "c", ts: Date.UTC(2026, 7, 14, 9, 5), sender: "simon", body: "morning", is_outgoing: false, deleted: false, edited: false, reactions: [], attachments: [] },
];

/** Catch-all first: Playwright runs handlers last-registered-first. */
async function openThread(page: Page): Promise<void> {
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONVERSATIONS }));
  await page.route("**/api/conversations/**/messages**", (r) =>
    r.fulfill({ json: { messages: MESSAGES, has_more: false, next_cursor: null } }),
  );
  await page.goto("/conversation/irc/7");
  await page.locator('.msg[data-id="c"] .body').waitFor();
}

/** Drag across two message bodies the way a finger or a mouse does, from a few
 *  pixels inside the first to a few pixels inside the last. */
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
  // ⚠ This is the check on `user-select: none`, and it needs the drag to START
  // ON THE META ROW — a body-to-body drag passes with the rule removed, because
  // the nick was never in the range to begin with. Grab the bubble from its top
  // corner, which is where a real one-message selection starts.
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
