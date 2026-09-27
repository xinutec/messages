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
 * Up from a conversation is the list, keeping the origin filter and dropping the
 * paged-back depth; the bar draws it (`@xinutec/ui-scaffold`).
 */
export const routes: Routes = [
  {
    path: 'conversation/:origin/:id',
    component: Thread,
    data: { up: { path: '/', keep: ['origin'], label: 'Back to conversations' } },
  },
  { path: '', component: Thread },
  { path: '**', redirectTo: '' },
];
