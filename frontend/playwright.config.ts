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
 * ⚠ What is NOT here, and why it is not an oversight: smoke/routing/
 * thread-scroll were written and tuned against `ng serve` and land differently
 * on a production build, so they keep their own dev-serve config in
 * playwright.behaviour.config.ts and run on demand. `copy` has no such
 * dependency — it was run against the LIVE deployment before being moved here.
 */
export default defineConfig(
  phoneConfig(harness, devices, { testMatch: '**/{ui-pages,copy}.spec.ts', goldens: true }),
);
