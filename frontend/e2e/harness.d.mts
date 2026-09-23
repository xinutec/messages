// Types for `harness.mjs`, which stays JavaScript because the harness's static
// server loads it under plain Node.
import type { HarnessSpec } from '@xinutec/ui-harness/config';

declare const spec: HarnessSpec;
export default spec;
