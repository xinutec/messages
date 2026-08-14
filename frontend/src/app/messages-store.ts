import { Injectable, inject, signal } from '@angular/core';

import { MessagesApi } from './messages-api';
import { Conversation, Me } from './models';

// Shared shell state: the signed-in user and the conversation list, loaded once
// and read by both the App shell (toolbar + list) and the routed Thread (title
// lookup). Keeping it here lets the Thread render from a deep link without the
// shell having to hand it data through the router.
@Injectable({ providedIn: 'root' })
export class MessagesStore {
  private api = inject(MessagesApi);

  readonly me = signal<Me | null>(null);
  readonly loading = signal(true);
  readonly conversations = signal<Conversation[]>([]);

  private started = false;

  /** Load the user then the conversation list. Idempotent (the shell calls it). */
  init(): void {
    if (this.started) return;
    this.started = true;
    this.api.me().subscribe({
      next: (m) => {
        this.me.set(m);
        this.loading.set(false);
        this.refresh();
      },
      error: () => {
        this.me.set(null);
        this.loading.set(false);
      },
    });
  }

  /** Re-read the conversation list.
   *
   *  ⚠ **Without this the list is loaded once and never again**, which showed
   *  up as a message count that disagreed with the database (14,435 against
   *  14,438) for an action the user had just taken himself. `init` is guarded
   *  so the shell can call it freely; that guard used to cover the fetch too.
   *
   *  Deliberately NOT on a timer. This query aggregates 3.7M rows and takes
   *  ~1.5s — see `archive.rs` — so it runs when returning to the list, which is
   *  the moment its answer is about to be looked at. The open thread polls for
   *  new messages separately, and that query is indexed and cheap.
   *
   *  Errors leave the previous list in place: a stale list beats an empty one,
   *  and the failure is visible the moment anything is tapped. */
  refresh(): void {
    this.api.conversations().subscribe({
      next: (cs) => this.conversations.set(cs),
      error: () => {},
    });
  }

  find(origin: string, id: string): Conversation | null {
    return this.conversations().find((c) => c.origin === origin && c.id === id) ?? null;
  }

  title(c: Conversation): string {
    // Empty/whitespace name → kind-based fallback. An explicit length check (not
    // `||`/`??`) makes the empty-string-is-no-name intent unambiguous.
    const name = c.name?.trim() ?? '';
    return name.length > 0 ? name : c.kind === 'dm' ? 'Direct message' : 'Group';
  }
}
