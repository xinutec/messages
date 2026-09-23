import { Routes } from '@angular/router';

import { Thread } from './thread';

/**
 * Routes, both rendering `Thread` in the shell's outlet:
 *
 *   /                            → the shell with an empty thread pane
 *   /conversation/:origin/:id    → that conversation's thread
 *
 * The origin filter and paged-back depth are query params (`?origin`, `?from`).
 * `withComponentInputBinding()` binds `:origin` and `:id` to the Thread's inputs.
 */
export const routes: Routes = [
  { path: 'conversation/:origin/:id', component: Thread },
  { path: '', component: Thread },
  { path: '**', redirectTo: '' },
];
