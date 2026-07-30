import { defineConfig, devices } from '@playwright/test';
import { phoneConfig } from '@xinutec/ui-harness/config';
import harness from './e2e/harness.mjs';

/**
 * Playwright UI-render checks — NOT behavioural unit tests. They render the app
 * in a real browser at true phone geometry and assert measurable facts about
 * the pixels (icon fonts actually load; no text overlaps; nothing overflows the
 * width). jsdom has no fonts or layout, so a mat-icon that silently falls back
 * to its ligature word ("search") reads green in vitest and only the render
 * disagrees.
 *
 * Everything shared — the Pixel geometry, the port, the static server serving
 * the PRODUCTION build, the golden tolerances — comes from @xinutec/ui-harness
 * (repo ~/Code/ui-harness); see dev-lint/docs/layout-quality-architecture.md.
 * What this app says about itself is in e2e/harness.mjs.
 *
 * Scope: this config runs ONLY the layout harness (ui-pages.spec.ts). The
 * app's behavioural specs (smoke/routing/thread-scroll) were written and tuned
 * against `ng serve` (dev) and land differently on a production build, so they
 * keep their own dev-serve config in playwright.behaviour.config.ts — run
 * on-demand via `npm run e2e:behaviour`, not part of the pre-push layout gate.
 */
export default defineConfig(
  phoneConfig(harness, devices, { testMatch: '**/ui-pages.spec.ts', goldens: true }),
);
