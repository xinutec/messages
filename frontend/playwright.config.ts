import { defineConfig, devices } from '@playwright/test';
import { phoneConfig } from '@xinutec/ui-harness/config';
import harness from './e2e/harness.mjs';

/**
 * The specs that must run in a real browser against the production build: the
 * gate's `ui-check` row.
 *
 * - `ui-pages`, `smoke`, `routing`, `thread-scroll`: render and scroll facts
 *   (fonts, overlap, overflow, geometry), which jsdom cannot see.
 * - `copy`: the Selection and Clipboard APIs, which jsdom gets wrong.
 *
 * Shared settings (Pixel geometry, port, static server, golden tolerances) come
 * from @xinutec/ui-harness (~/Code/ui-harness); see
 * dev-lint/docs/layout-quality-architecture.md. This app's own are in
 * e2e/harness.mjs.
 */
export default defineConfig(
  phoneConfig(harness, devices, { testMatch: '**/*.spec.ts', goldens: true }),
);
