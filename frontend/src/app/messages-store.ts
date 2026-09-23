import { Injectable, inject, signal } from '@angular/core';

import { MessagesApi } from './messages-api';
import { Conversation, ConversationKind, Me } from './models';

// Shell state, the signed-in user and the conversation list, shared by the
// shell and the Thread, so a deep-linked Thread needs nothing from the router.
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

  /** Re-read the conversation list, when the reader returns to it. On error the
   *  previous list stays: a stale list beats an empty one. */
  refresh(): void {
    this.api.conversations().subscribe({
      next: (cs) => this.conversations.set(cs),
      error: () => {},
    });
  }

  find(origin: string, id: string): Conversation | null {
    return this.conversations().find((c) => c.origin === origin && c.id === id) ?? null;
  }

  /** What to call a conversation with no name, per kind. */
  private readonly unnamed: Record<ConversationKind, string> = {
    dm: 'Direct message',
    group: 'Group',
    channel: 'Channel',
  };

  title(c: Conversation): string {
    // An empty or whitespace name is no name.
    const name = c.name?.trim() ?? '';
    return name.length > 0 ? name : this.unnamed[c.kind];
  }
}
