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
        this.api.conversations().subscribe((cs) => this.conversations.set(cs));
      },
      error: () => {
        this.me.set(null);
        this.loading.set(false);
      },
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
