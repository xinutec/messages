// Sending on IRC or Telegram: the draft, kept per conversation, the send, and
// the answer of irssi or of the Telegram feed.

import { type Signal, signal } from '@angular/core';
import { firstValueFrom } from 'rxjs';

import { MessagesApi } from './messages-api';
import { Origin } from './models';

export class Composer {
  readonly draft = signal('');
  /** An IME candidate is in flight. `send` refuses then, since Enter in a form
   *  submits even when the keydown handler returned early. */
  readonly composing = signal(false);
  readonly sending = signal(false);
  readonly error = signal<string | null>(null);
  /** What was left mid-sentence in other conversations, by `origin:id`, as
   *  Signal keeps it; never sent anywhere but where it was typed. */
  private readonly drafts = new Map<string, string>();
  private key: string | null = null;

  constructor(
    private readonly api: MessagesApi,
    /** The open conversation, or null. */
    private readonly where: Signal<{ origin: Origin; id: string } | null>,
    /** Something went and is archived: show it. */
    private readonly sent: () => Promise<void>,
  ) {}

  /** The conversation changed: this draft waits for its own, the new one's
   *  comes back. */
  switchTo(key: string | null): void {
    if (this.key != null) {
      const left = this.draft();
      if (left) this.drafts.set(this.key, left);
      else this.drafts.delete(this.key);
    }
    this.key = key;
    this.draft.set(key == null ? '' : (this.drafts.get(key) ?? ''));
    this.error.set(null);
  }

  async send(): Promise<void> {
    const w = this.where();
    const text = this.draft().trim();
    if (!w || !text || this.sending() || this.composing()) return;
    // IRC carries one line per message, so a paragraph goes as its lines, in
    // order, as every client sends a paste; Telegram carries the breaks.
    const lines = w.origin === 'irc' ? text.split('\n').map((l) => l.trim()).filter(Boolean) : [text];
    const asked = lines.length;

    this.sending.set(true);
    this.error.set(null);
    let archived = true;
    try {
      while (lines.length > 0) {
        const res = await firstValueFrom(this.api.send(w.origin, w.id, lines[0]));
        if (!res.sent) {
          this.error.set(res.error ?? 'Not sent.');
          break;
        }
        lines.shift();
        archived &&= res.archived;
      }
    } catch {
      this.error.set('Could not reach the server.');
    } finally {
      this.sending.set(false);
    }
    // What went leaves the box; what was refused stays, to try again.
    this.draft.set(lines.join('\n'));
    if (lines.length === asked) return;
    if (archived) await this.sent();
    else if (this.error() == null) {
      this.error.set(w.origin === 'irc' ? 'Sent. It will appear here after the next import.' : 'Sent, but not archived yet.');
    }
  }

  /** At a desk Enter sends and Shift+Enter breaks the line; on a touch screen
   *  the return key breaks the line and the button sends, as in Signal. */
  onKey(e: KeyboardEvent): void {
    // An IME is composing: Enter accepts its candidate. `keyCode 229` is how
    // older Android WebViews report it.
    if (e.isComposing || e.keyCode === 229) return;
    if (e.key === 'Enter' && !e.shiftKey && !touchScreen()) {
      e.preventDefault();
      void this.send();
    }
  }
}

function touchScreen(): boolean {
  return typeof matchMedia === 'function' && matchMedia('(pointer: coarse)').matches;
}
