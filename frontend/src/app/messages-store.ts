import { HttpErrorResponse } from '@angular/common/http';
import { Injectable, inject, signal } from '@angular/core';

import { MessagesApi } from './messages-api';
import { Conversation, ConversationKind, Me, Origin } from './models';

/** Where the last-known identity is kept, so the app opens offline rather than
 *  at sign-in; the service worker keeps the list and the threads (ngsw-config). */
const ME_KEY = 'messages.me';

// Shell state, the signed-in user and the conversation list, shared by the
// shell and the Thread, so a deep-linked Thread needs nothing from the router.
@Injectable({ providedIn: 'root' })
export class MessagesStore {
  private api = inject(MessagesApi);
  private readonly remembered = loadRememberedMe();
  readonly me = signal<Me | null>(this.remembered);
  /** Only a cold start with no remembered identity blocks on the loader. */
  readonly loading = signal(this.remembered === null);
  /** `/api/me` failed other than by being signed out, with no remembered
   *  identity: offline, not sign-in. */
  readonly offline = signal(false);
  readonly conversations = signal<Conversation[]>([]);
  /** The list has had an answer: what the worker kept, or the network's, or its
   *  failure. Before that an empty list is not yet known to be empty. */
  readonly listKnown = signal(false);
  /** The last word from the network was no network: what is on view is what
   *  the service worker kept, and the shell says so. Cleared by any answer. */
  readonly stale = signal(false);
  private started = false;
  /** The network has answered for the list; what the worker kept no longer counts. */
  private fresh = false;

  noteAnswer(): void {
    this.stale.set(false);
  }

  /** A request failed. Only no network counts as stale: a server fault is
   *  not "what was last seen". */
  noteFailure(e: unknown): void {
    if (noNetwork(e)) this.stale.set(true);
  }
  /** Load the user then the conversation list. Idempotent (the shell calls it). */
  init(): void {
    if (this.started) return;
    this.started = true;
    // The list as last seen, at once; the network's replaces it.
    void this.api.keptConversations().then((kept) => {
      if (kept != null && !this.fresh) {
        this.conversations.set(kept);
        this.listKnown.set(true);
      }
    });
    this.api.me().subscribe({
      next: (m) => {
        this.me.set(m);
        rememberMe(m);
        this.offline.set(false);
        this.noteAnswer();
        this.loading.set(false);
        this.refresh();
      },
      error: (e: unknown) => {
        if (signedOut(e)) {
          this.me.set(null);
          rememberMe(null);
        } else {
          // The network or the server, not a sign-out: the remembered identity
          // stands, and the list comes from the service worker's cache if it has it.
          this.offline.set(true);
          this.noteFailure(e);
          if (this.me() != null) this.refresh();
        }
        this.loading.set(false);
      },
    });
  }

  /** Leaving: forget who it was, so the next start asks. */
  forget(): void {
    rememberMe(null);
    this.me.set(null);
  }

  /** Re-read the conversation list, when the reader returns to it. On error the
   *  previous list stays: a stale list beats an empty one. */
  refresh(): void {
    this.api.conversations().subscribe({
      next: (cs) => {
        this.fresh = true;
        this.noteAnswer();
        this.conversations.set(cs);
        this.listKnown.set(true);
      },
      error: (e: unknown) => {
        this.noteFailure(e);
        this.listKnown.set(true);
      },
    });
  }

  find(origin: Origin, id: string): Conversation | null {
    return this.conversations().find((c) => c.origin === origin && c.id === id) ?? null;
  }

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

/** A positive sign-out: the server said so. A dropped connection, the service
 *  worker's offline answer (504) or a server fault is not one. */
function signedOut(e: unknown): boolean {
  return e instanceof HttpErrorResponse && (e.status === 401 || e.status === 403);
}

/** A dropped connection (status 0) or the service worker's offline answer (504). */
function noNetwork(e: unknown): boolean {
  return e instanceof HttpErrorResponse && (e.status === 0 || e.status === 504);
}

function isMe(v: unknown): v is Me {
  return (
    typeof v === 'object' &&
    v !== null &&
    'user_id' in v &&
    typeof v.user_id === 'string' &&
    'display_name' in v &&
    typeof v.display_name === 'string'
  );
}

function loadRememberedMe(): Me | null {
  try {
    const raw = localStorage.getItem(ME_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    return isMe(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function rememberMe(me: Me | null): void {
  try {
    if (me) localStorage.setItem(ME_KEY, JSON.stringify(me));
    else localStorage.removeItem(ME_KEY);
  } catch {
    // Storage denied or full: the next start asks the server, as before.
  }
}
