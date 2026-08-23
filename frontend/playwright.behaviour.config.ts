import { defineConfig, devices } from "@playwright/test";

/**
 * Behavioural e2e — the app's scroll/routing/render-soundness specs
 * (smoke, routing, thread-scroll). These are NOT the gate's browser suite
 * (that's playwright.config.ts: ui-pages + copy); they assert behaviour —
 * sticky headers, URL contracts, landing at the newest message.
 *
 * ⚠ **Being here means running only when someone remembers.** That is the cost
 * of the dev-serve dependency below, not a judgement that these matter less. A
 * spec that does NOT need `ng serve` belongs in playwright.config.ts, where the
 * gate runs it — `copy` moved there for exactly that reason.
 *
 * Same Pixel 7 phone geometry as the layout harness (a real device preset, not
 * the old 390px "Desktop Chrome" spread — that antipattern rendered tighter
 * line boxes and produced phantom overlaps). The one difference from the layout
 * config: these run against `ng serve` (dev), not the production build via
 * serve.mjs. thread-scroll's "lands at the newest message after image reflow"
 * re-pin fires on a different tick under a production build and only lands
 * reliably under the dev server it was written against. Run on-demand:
 *   pnpm run e2e:behaviour
 * They are not part of the pre-push layout gate (they never were).
 */
export default defineConfig({
  testDir: "./e2e",
  testMatch: /(smoke|routing|thread-scroll)\.spec\.ts/,
  fullyParallel: true,
  reporter: [["list"]],
  use: {
    baseURL: "http://localhost:4200",
    screenshot: "only-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Pixel 7"], deviceScaleFactor: 1 } }],
  webServer: {
    command: "npm start",
    url: "http://localhost:4200",
    reuseExistingServer: true,
    timeout: 120_000,
  },
});
