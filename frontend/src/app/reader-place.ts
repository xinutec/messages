// Where the reader is in a conversation, and why: put there by the app (a
// search hit, a reply, a picked day) or by their own scrolling. One owner for
// the landing's marker, whether the loaded window reaches the present, and
// what the address says; the thread asks it rather than keeping flags.

import { signal } from '@angular/core';
import { ParamMap } from '@angular/router';

/** What the address's query should change to; `null` drops a key. */
export type AddressChange = Partial<Record<'from' | 'at' | 'on', string | null>>;

export class ReaderPlace {
  /** The message the reader was put on, marked so it can be told from its
   *  neighbours; null when there is none, and once they scroll. */
  readonly landedId = signal<string | null>(null);

  /** The loaded window stops short of the present: a landing in the past, or
   *  a gap the poll found. Polling waits, and scrolling down fetches forwards
   *  until the window reaches the present. */
  readonly floating = signal(false);

  /** The reader has scrolled since the thread opened or last landed: the
   *  landing (its marker, `?at`, `?on`) is behind them. */
  private scrolled = false;

  /** The landing the address last named, so a change can be told from the
   *  thread clearing it. */
  private lastAt: string | null;
  private lastOn: string | null;

  constructor(opened: ParamMap) {
    this.lastAt = opened.get('at');
    this.lastOn = opened.get('on');
  }

  reset(): void {
    this.landedId.set(null);
    this.floating.set(false);
    this.scrolled = false;
  }

  /** Put on a message, or near it (`id` null when the place had none), with a
   *  window that reaches the present or stops short of it. */
  landOn(id: string | null, stopsShort: boolean): void {
    this.landedId.set(id);
    this.floating.set(stopsShort);
    this.scrolled = false;
  }

  /** Whether the loaded window reaches the present now: a page fetched
   *  forwards, or a poll that found a gap. */
  reachesPresent(yes: boolean): void {
    this.floating.set(!yes);
  }

  readerScrolled(): void {
    this.scrolled = true;
    this.landedId.set(null);
  }

  get hasScrolled(): boolean {
    return this.scrolled;
  }

  /** What the address should say for the view at `fromTs` (null: at the end),
   *  or null when it already does. The landing stays in it until the reader
   *  scrolls: the pages the app loads around it scroll too. */
  address(now: ParamMap, fromTs: number | null): AddressChange | null {
    const from = fromTs != null ? String(fromTs) : null;
    const landingGone = !this.scrolled || (now.get('at') == null && now.get('on') == null);
    if (now.get('from') === from && landingGone) return null;
    return this.scrolled ? { from, at: null, on: null } : { from };
  }

  /** Whether the address now names a landing other than the last: a changed
   *  `?at` or `?on`. The thread clearing them is not one. */
  isNewLanding(now: ParamMap): boolean {
    const at = now.get('at');
    const on = now.get('on');
    const moved = (at != null && at !== this.lastAt) || (on != null && on !== this.lastOn);
    this.lastAt = at;
    this.lastOn = on;
    return moved;
  }
}
