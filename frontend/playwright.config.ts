import { defineConfig, devices } from '@playwright/test';
import { phoneConfig } from '@xinutec/ui-harness/config';
import harness from './e2e/harness.mjs';

/**
 * The specs that must run in a REAL BROWSER, against the production build —
 * this is the gate's `ui-check` row, so what is here runs on every commit.
 *
 * Two kinds, admitted for the same reason: jsdom cannot answer the question.
 *
 * - `ui-pages` — render facts. Icon fonts actually load, no text overlaps,
 *   nothing overflows the width. jsdom has no fonts and no layout, so a
 *   mat-icon that falls back to its ligature word ("search") reads green in
 *   vitest and only the render disagrees.
 * - `copy` — the Selection and Clipboard APIs. Measured 2026-08-16: jsdom's
 *   `Selection.containsNode` answered true for a node past the range AND false
 *   for the node a selection sat inside. The vitest specs for copy are the
 *   convenience; these are the evidence, so they cannot be the ones nobody
 *   runs.
 *
 * Everything shared — the Pixel geometry, the port, the static server serving
 * the PRODUCTION build, the golden tolerances — comes from @xinutec/ui-harness
 * (repo ~/Code/ui-harness); see dev-lint/docs/layout-quality-architecture.md.
 * What this app says about itself is in e2e/harness.mjs.
 *
 * ⚠ **`smoke`, `routing` and `thread-scroll` used to live in a second config**
 * (`playwright.behaviour.config.ts`, deleted 2026-09-03) because they were
 * written against `ng serve` and thread-scroll's image-load re-pin was said to
 * "fire on a different tick under a production build". That verdict was
 * re-tested rather than inherited: all three pass here against the production
 * build, thread-scroll 5/5 on repeat and the trio 45/45. They mock every
 * `/api/**` call, so they never needed the dev server's proxy — the whole suite
 * is 31 tests in ~6s.
 *
 * The cost of the old arrangement was that the app's subtlest code — the scroll
 * windowing engine — had its only browser coverage in the config nobody runs.
 */
export default defineConfig(
  phoneConfig(harness, devices, { testMatch: '**/*.spec.ts', goldens: true }),
);
