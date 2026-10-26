// Sending on IRC, Telegram or Signal: the draft, kept per conversation, the
// send, and the answer of irssi, the Telegram feed or the Signal ingester.

import { type Signal, signal } from '@angular/core';
import { firstValueFrom } from 'rxjs';

import { MessagesApi } from './messages-api';
import { Message, Origin } from './models';

export class Composer {
  readonly draft = signal('');
  /** An IME candidate is in flight. `send` refuses then, since Enter in a form
   *  submits even when the keydown handler returned early. */
  readonly composing = signal(false);
  readonly sending = signal(false);
  /** Why the last send did not go; the draft is kept to try again. */
  readonly error = signal<string | null>(null);
  /** What a send that went still has to say: it is not archived yet, or a
   *  Signal group member did not get it. */
  readonly notice = signal<string | null>(null);
  /** The Telegram or Signal message the next send answers. IRC has no replies:
   *  there an answer is the draft starting with the nick, as every client
   *  writes one. */
  readonly replyingTo = signal<Message | null>(null);
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
    this.notice.set(null);
    this.replyingTo.set(null);
  }

  /** Answer `m`: on Telegram and Signal the send will quote it; on IRC the
   *  draft is addressed to its sender, once. */
  replyTo(m: Message, origin: Origin): void {
    if (origin === 'irc') {
      const to = `${m.sender}: `;
      if (!this.draft().startsWith(to)) this.draft.set(to + this.draft());
    } else {
      this.replyingTo.set(m);
    }
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
    this.notice.set(null);
    let archived = true;
    // What a send that went still has to say, such as who it missed.
    let note: string | null = null;
    try {
      while (lines.length > 0) {
        const res = await firstValueFrom(this.api.send(w.origin, w.id, lines[0], this.replyingTo()?.id));
        if (!res.sent) {
          this.error.set(res.error ?? 'Not sent.');
          break;
        }
        lines.shift();
        archived &&= res.archived;
        note = res.note ?? note;
        // The answer went: what follows is a message of its own.
        this.replyingTo.set(null);
      }
    } catch {
      this.error.set('Could not reach the server.');
    } finally {
      this.sending.set(false);
    }
    // What went leaves the box; what was refused stays, to try again.
    this.draft.set(lines.join('\n'));
    if (lines.length === asked) return;
    if (!archived) {
      note ??= w.origin === 'irc' ? 'Sent. It will appear here after the next import.' : 'Sent, but not archived yet.';
    }
    this.notice.set(note);
    if (archived) await this.sent();
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
