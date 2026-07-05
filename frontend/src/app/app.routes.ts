import { Routes } from '@angular/router';

import { Thread } from './thread';

/**
 * Routes for the SPA — a real table (like the health/life apps), not a single
 * catch-all. The open conversation is a routed resource rendered in the shell's
 * `<router-outlet>`:
 *
 *   /                            → the shell with an empty thread pane ("pick a conversation")
 *   /conversation/:origin/:id    → that conversation's thread
 *
 * Both map to the same `Thread` component (empty → placeholder), the way health
 * shares one component across `''` and `share/:token`. The origin filter and the
 * paged-back depth stay query params (`?origin` / `?from`) — they're view-state
 * layered on a route, not navigation. `withComponentInputBinding()` (app.config)
 * binds `:origin`/`:id` straight to the Thread's inputs.
 */
export const routes: Routes = [
  { path: 'conversation/:origin/:id', component: Thread },
  { path: '', component: Thread },
  { path: '**', redirectTo: '' },
];
