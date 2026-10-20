// What the thread's DOM window (thread-window.ts `MAX_RENDERED`) saves: page
// back through a conversation that never runs out, at 4x CPU throttling as a
// stand-in for a phone, and print the cost at each depth. A measurement, not a
// gate: run `pnpm run perf` against each build to compare, e.g. with the cap
// set out of reach. Text only, as an IRC channel is; pictures cost more.
import { test, type Page } from "@playwright/test";

import type { Conversation, Me, Message, MessagesPage } from "../src/app/models";

declare global {
  interface Window {
    /** Long main-thread tasks since last cleared, in ms. */
    longTasks: number[];
  }
}

const ME: Me = { user_id: "reader", display_name: "Reader" };
const CONV: Conversation[] = [{ origin: "irc", id: "7", name: "#busy", kind: "channel", network: "net",
  message_count: 100000, last_ts: 1_800_000_000_000, last: null, unread: 0, avatar: null }];
const WORDS = "the quick brown fox jumps over a lazy dog while somebody asks about the build and another replies".split(" ");
function body(i: number): string {
  const n = 3 + ((i * 7919) % 40);
  let s = ""; for (let k = 0; k < n; k++) s += WORDS[(i + k * 31) % WORDS.length] + " ";
  if (i % 17 === 0) s += "\nsecond line of a longer paragraph that wraps on a phone screen";
  return s.trim();
}
function msg(i: number): Message {
  return { id: String(i), ts: 1_700_000_000_000 + i * 60_000, sender: "nick" + (i % 9), sender_key: "nick" + (i % 9),
    is_outgoing: i % 5 === 0, kind: "message", body: body(i), deleted: false, edited: false,
    reactions: i % 13 === 0 ? [{ emoji: "👍", count: 2, who: [] }] : [], attachments: [], link_images: [], edits: [],
    link_offers: [], reply_to: null, delivery: null, entities: [], album: null, previews: [] };
}
const TOP = 100_000;
async function mock(page: Page, served: { pages: number }) {
  await page.route("**/api/**", (r) => r.fulfill({ status: 204, body: "" }));
  await page.route("**/api/me", (r) => r.fulfill({ json: ME }));
  await page.route("**/api/conversations", (r) => r.fulfill({ json: CONV }));
  await page.route("**/api/conversations/irc/7/messages**", (r) => {
    const c = new URL(r.request().url()).searchParams.get("cursor");
    const end = c ? Number(c) : TOP;
    const ms = []; for (let i = end - 100; i < end; i++) ms.push(msg(i));
    served.pages++;
    return r.fulfill({ json: { messages: ms, has_more: true, next_cursor: String(end - 100), prev_cursor: String(end) } satisfies MessagesPage });
  });
}

async function loadOlderTo(page: Page, served: { pages: number }, pages: number) {
  while (served.pages < pages) {
    const before = served.pages;
    await page.evaluate(() => { const t = document.querySelector(".thread")!; t.dispatchEvent(new WheelEvent("wheel", { bubbles: true })); t.scrollTop = 0; });
    for (let k = 0; k < 100 && served.pages === before; k++) await page.waitForTimeout(20);
    await page.waitForTimeout(60);
  }
}

test("what the DOM window saves, by depth", async ({ page }) => {
  test.setTimeout(900_000);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Performance.enable");
  const served = { pages: 0 };
  await mock(page, served);
  await page.addInitScript(() => {
    window.longTasks = [];
    new PerformanceObserver((l) => { for (const e of l.getEntries()) window.longTasks.push(e.duration); }).observe({ type: "longtask", buffered: true });
  });
  await page.goto("/conversation/irc/7");
  await page.locator(".msg").first().waitFor();
  await page.waitForTimeout(500);
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
  const rows: string[] = [];
  for (const target of [4, 10, 30, 60]) {
    await loadOlderTo(page, served, target);
    await page.waitForTimeout(800);
    // One more page, timed: main-thread long tasks while it lands.
    await page.evaluate(() => (window.longTasks = []));
    const t0 = Date.now();
    const before = served.pages;
    await page.evaluate(() => { const t = document.querySelector(".thread")!; t.dispatchEvent(new WheelEvent("wheel", { bubbles: true })); t.scrollTop = 0; });
    while (served.pages === before) await page.waitForTimeout(10);
    await page.waitForTimeout(1500);
    const longs = await page.evaluate(() => window.longTasks);
    const landMs = longs.reduce((a, b) => a + b, 0);
    const worstLand = Math.max(0, ...longs);
    // Scrolling: 90 frames of 30px, frame times recorded.
    const frames: number[] = await page.evaluate(async () => {
      const t = document.querySelector<HTMLElement>(".thread")!;
      t.scrollTop = Math.max(t.scrollTop, 6000);
      const out: number[] = []; let last = performance.now();
      for (let i = 0; i < 90; i++) {
        t.dispatchEvent(new WheelEvent("wheel", { bubbles: true }));
        t.scrollTop -= 30;
        await new Promise((r) => requestAnimationFrame(() => r(null)));
        const now = performance.now(); out.push(now - last); last = now;
      }
      return out;
    });
    frames.sort((a, b) => a - b);
    const p95 = frames[Math.floor(frames.length * 0.95)];
    const slow = frames.filter((f) => f > 50).length;
    await cdp.send("HeapProfiler.collectGarbage");
    const heap = await cdp.send("Runtime.getHeapUsage");
    const dom = await page.evaluate(() => ({ nodes: document.getElementsByTagName("*").length, msgs: document.querySelectorAll(".msg").length }));
    rows.push(`loaded=${served.pages * 100} rendered=${dom.msgs} domNodes=${dom.nodes} heapMB=${(heap.usedSize / 1e6).toFixed(1)} pageLandLongTaskMs=${landMs.toFixed(0)} worstTaskMs=${worstLand.toFixed(0)} scrollP95ms=${p95.toFixed(1)} framesOver50ms=${slow}/90 (${Date.now() - t0}ms)`);
    console.log(rows[rows.length - 1]);
  }
});
