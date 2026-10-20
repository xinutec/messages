import { defineConfig, devices } from '@playwright/test';
import { phoneConfig } from '@xinutec/ui-harness/config';
import harness from './e2e/harness.mjs';

/** Measurements, not gates: `pnpm run perf`. The same harness as the browser
 *  suite, pointed at `perf/`. */
export default defineConfig({
  ...phoneConfig(harness, devices, { testMatch: '**/*.perf.ts' }),
  testDir: './perf',
});
