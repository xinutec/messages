// Sending a line, IRC only: the draft, the send, and irssi's answer.

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
  /** irssi's refusal, shown as-is. */
  readonly error = signal<string | null>(null);

  constructor(
    private readonly api: MessagesApi,
    private readonly origin: Signal<Origin | undefined>,
    private readonly id: Signal<string | undefined>,
    /** irssi logged it: show the line, by reloading the thread. */
    private readonly archived: () => Promise<void>,
  ) {}

  async send(): Promise<void> {
    const o = this.origin();
    const i = this.id();
    const text = this.draft().trim();
    if (o == null || i == null || !text || this.sending() || this.composing()) return;

    this.sending.set(true);
    this.error.set(null);
    try {
      const res = await firstValueFrom(this.api.send(o, i, text));
      if (!res.sent) {
        this.error.set(res.error ?? 'Not sent.');
        return;
      }
      // Cleared only once sent, so a failure keeps what was typed.
      this.draft.set('');
      if (res.archived) await this.archived();
      else this.error.set('Sent. It will appear here after the next import.');
    } catch {
      this.error.set('Could not reach the server.');
    } finally {
      this.sending.set(false);
    }
  }

  /** Enter sends. The box is single-line: IRC cannot carry a newline. */
  onKey(e: KeyboardEvent): void {
    // An IME is composing: Enter accepts its candidate. `keyCode 229` is how
    // older Android WebViews report it.
    if (e.isComposing || e.keyCode === 229) return;
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      void this.send();
    }
  }
}
