import { defineConfig, devices } from '@playwright/test';

/**
 * Playwright UI-render checks — NOT behavioural unit tests. They render the app
 * in a real browser at true phone geometry and assert measurable facts about
 * the pixels (icon fonts actually load; no text overlaps; nothing overflows the
 * width). jsdom has no fonts or layout, so a mat-icon that silently falls back
 * to its ligature word ("search") reads green in vitest and only the render
 * disagrees. Shared checkers live in @xinutec/ui-harness (repo ~/Code/ui-harness); see
 * dev-lint/docs/layout-quality-architecture.md.
 *
 * Runs against the PRODUCTION build served statically by e2e/serve.mjs — one
 * device, identical to life/fleetwatch. `npm run ui-check` (wired into
 * verify.sh after `ng build`) serves the freshly-built dist; reuseExistingServer
 * attaches to a serve.mjs already up.
 *
 * Scope: this config runs ONLY the layout harness (ui-pages.spec.ts). The
 * app's behavioural specs (smoke/routing/thread-scroll) were written and tuned
 * against `ng serve` (dev) and land differently on a production build, so they
 * keep their own dev-serve config in playwright.behaviour.config.ts — run
 * on-demand via `npm run e2e:behaviour`, not part of the pre-push layout gate.
 */
const PORT = 4272;

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/ui-pages.spec.ts',
  reporter: [['list']],
  // One committed baseline per name — no {projectName}/{platform} suffix; these
  // only ever run on one machine (a dev's Mac; CI runs Rust/unit only, never
  // Playwright). See the CI split in the architecture doc.
  snapshotPathTemplate: 'e2e/__screenshots__/{arg}{ext}',
  expect: {
    toHaveScreenshot: {
      maxDiffPixelRatio: 0.01,
      animations: 'disabled',
      caret: 'hide',
    },
  },
  timeout: 90_000,
  use: {
    baseURL: `http://localhost:${PORT}`,
    screenshot: 'only-on-failure',
  },
  // Emulate the phone this app is actually used on (Pixel 9 ≈ the Pixel 7
  // preset: 412 CSS px wide, mobile UA, touch). The viewport MUST live in the
  // PROJECT `use`, not the global one: a device spread carries its own
  // viewport, and project-level `use` overrides global — which is exactly how
  // an earlier `...devices['Desktop Chrome']` here silently ran every "phone
  // width" test at desktop width. deviceScaleFactor is forced to 1 so CSS-pixel
  // geometry (what the layout checks measure) is DPR-invariant and small.
  projects: [{ name: 'chromium', use: { ...devices['Pixel 7'], deviceScaleFactor: 1 } }],
  webServer: {
    command: `node e2e/serve.mjs ${PORT}`,
    url: `http://localhost:${PORT}/`,
    reuseExistingServer: true,
    timeout: 60_000,
  },
});
