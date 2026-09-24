// Keeping a long conversation's DOM bounded while it scrolls as one thread.
//
// Every fetched message is retained, but only a window is rendered; spacers
// stand in for the runs collapsed off either end. Spacer heights are averages,
// so the scrollbar is approximate; what the reader sees stays exact because
// every mutation re-anchors the viewport on the message at its top.

import { Signal, computed, signal } from '@angular/core';

import { Message } from './models';

/** Server page size, and so the unit a run is collapsed or revealed in. */
export const PAGE = 100;
/** Soft cap on how many messages are ever in the DOM. */
export const MAX_RENDERED = 400;
/** How many pages `loadThread` fetches at most to restore a `?from` depth:
 *  five times `MAX_RENDERED`, beyond any legitimate restore. */
export const MAX_RESTORE_PAGES = (MAX_RENDERED * 5) / PAGE;
/** How close (px) to an edge before revealing or loading, far enough ahead that
 *  the reader rarely sees a blank spacer. */
const EDGE = 1200;
/** A row's height before any is measured; affects only spacer sizes. */
const ROW_GUESS = 64;
/** How close (px) to the bottom counts as at the latest; small, so a short
 *  thread is not always at the bottom. */
const BOTTOM_EPS = 64;

/** A run of messages collapsed out of the DOM, and the height it occupied. */
interface Chunk {
  count: number;
  height: number;
}

const sumCount = (cs: Chunk[]): number => cs.reduce((a, c) => a + c.count, 0);
const sumHeight = (cs: Chunk[]): number => cs.reduce((a, c) => a + c.height, 0);

/** Whether scroll diagnostics are on. Guarded: localStorage and location can
 *  throw in sandboxed iframes. */
function readDebugFlag(): boolean {
  try {
    if (/(?:^|[?&])scrolldebug\b/.test(location.search)) return true;
    return localStorage.getItem('threadScrollDebug') === '1';
  } catch {
    return false;
  }
}

export class ThreadWindow {
  /** Older messages collapsed above the window; newer ones below. The last chunk
   *  of each is nearest the window. */
  private readonly above = signal<Chunk[]>([]);
  private readonly below = signal<Chunk[]>([]);

  private readonly lo = computed(() => sumCount(this.above()));
  private readonly end = computed(() => this.messages().length - sumCount(this.below()));

  /** The messages currently in the DOM (a window over the retained list). */
  readonly rendered = computed(() => this.messages().slice(this.lo(), this.end()));
  readonly renderCount = computed(() => this.end() - this.lo());
  readonly topSpacer = computed(() => sumHeight(this.above()));
  readonly bottomSpacer = computed(() => sumHeight(this.below()));

  /** Set while we move scrollTop ourselves, so a programmatic scroll can be told
   *  from the reader's. */
  private adjusting = false;
  get busy(): boolean {
    return this.adjusting;
  }

  /** Whether the reader is following the end of the conversation. Recorded on
   *  every scroll, because after a resize `atBottom()` answers for the new
   *  geometry. Starts true: a conversation opens at its latest message. */
  private following = true;

  /** The container height at the last scroll, so a scroll the resize caused is
   *  not taken for the reader's. Null until the first scroll, which has nothing
   *  to have resized from. */
  private hostHeight: number | null = null;
  /** The content height likewise, so growth below a reader at the end is not
   *  taken for them leaving. */
  private hostScrollHeight: number | null = null;

  /** The observer behind `observeShrink`, and the message block it watches. */
  private ro: ResizeObserver | null = null;
  private watched: HTMLElement | null = null;

  // Scroll diagnostics, off by default: enable with
  // `localStorage.threadScrollDebug = '1'` or `?scrolldebug`, then read the
  // `[thread-scroll]` debug lines. `jump` means visible content shifted on its
  // own; an operation line shows how far that step re-anchored.
  private readonly dbg = readDebugFlag();
  private lastAnchor: { id: string; top: number } | null = null;
  private lastScrollTop = 0;

  constructor(
    private readonly host: HTMLElement,
    /** The message container; a getter, since `viewChild` resolves after
     *  construction and after each render. */
    private readonly container: () => HTMLElement | undefined,
    private readonly messages: Signal<Message[]>,
    /** Flush pending renders, so the DOM can be measured. */
    private readonly tick: () => void,
  ) {}

  /** Back to "nothing collapsed" — for a fresh conversation. */
  reset(): void {
    this.above.set([]);
    this.below.set([]);
    this.following = true;
  }

  /** Advance the window for the current scroll. `needOlder` and `needNewer` tell
   *  the caller an edge is near with nothing collapsed there: fetch a page. */
  step(): { needOlder: boolean; needNewer: boolean } {
    const el = this.container();
    if (!el) return { needOlder: false, needNewer: false };
    if (this.dbg) this.detectJump();

    // Measured from rects: the host is not necessarily the offsetParent.
    const hostRect = this.host.getBoundingClientRect();
    const elRect = el.getBoundingClientRect();
    const nearTop = elRect.top - hostRect.top >= -EDGE;
    const nearBottom = elRect.bottom - hostRect.bottom <= EDGE;

    let needOlder = false;
    let needNewer = false;
    if (nearTop) {
      if (this.above().length) {
        this.revealTop();
        this.enforceMax('bottom');
      } else {
        needOlder = true;
      }
    }
    if (nearBottom) {
      if (this.below().length) {
        this.revealBottom();
        this.enforceMax('top');
      } else {
        needNewer = true;
      }
    }
    // Re-baseline after windowing, which legitimately re-anchors.
    if (this.dbg) {
      this.lastAnchor = this.topAnchor();
      this.lastScrollTop = this.host.scrollTop;
    }
    return { needOlder, needNewer };
  }

  /** Record whether the reader is following the end, on every scroll including
   *  our own: it is about where the viewport is, not who moved it. Separate from
   *  `step`, which is skipped while busy. A scroll caused by the container
   *  resizing does not count as the reader leaving. */
  noteScroll(): void {
    const h = this.host.clientHeight;
    const sh = this.host.scrollHeight;
    const grew = this.hostScrollHeight == null ? 0 : Math.max(0, sh - this.hostScrollHeight);
    const resized = this.hostHeight != null && h !== this.hostHeight;
    this.hostHeight = h;
    this.hostScrollHeight = sh;
    if (resized) return;
    // Content growing below a reader at the end (an image loading) is not the
    // reader leaving: they are off by no more than the growth.
    if (this.following && grew > 0 && sh - this.host.scrollTop - h <= grew + BOTTOM_EPS) return;
    this.following = this.atBottom();
  }

  /** Nothing collapsed below, and within a few pixels of the bottom. */
  atBottom(): boolean {
    return (
      this.below().length === 0 &&
      this.host.scrollHeight - this.host.scrollTop - this.host.clientHeight <= BOTTOM_EPS
    );
  }

  /** Collapse the far end until the DOM is under the cap. Call after growing the
   *  opposite end. */
  enforceMax(side: 'top' | 'bottom'): void {
    let guard = 0;
    while (this.renderCount() > MAX_RENDERED && guard++ < 64) {
      if (side === 'bottom') this.collapseBottom(PAGE);
      else this.collapseTop(PAGE);
    }
  }

  /** Collapse off-screen ends until the DOM is under the cap, after a restore
   *  rendered many pages. */
  trimToWindow(): void {
    let guard = 0;
    while (this.renderCount() > MAX_RENDERED && guard++ < 128) {
      const { above, below } = this.offscreenCounts();
      if (below >= above && below > 0) this.collapseBottom(Math.min(PAGE, below));
      else if (above > 0) this.collapseTop(Math.min(PAGE, above));
      else break; // nothing off-screen to collapse (viewport bigger than cap)
    }
  }

  private revealTop(): void {
    const cs = this.above();
    if (!cs.length) return;
    this.keepingAnchor('revealTop', () => this.above.set(cs.slice(0, -1)));
  }

  private revealBottom(): void {
    const cs = this.below();
    if (!cs.length) return;
    this.keepingAnchor('revealBottom', () => this.below.set(cs.slice(0, -1)));
  }

  private collapseTop(count: number): void {
    const n = Math.min(count, this.renderCount());
    if (n <= 0) return;
    const height = this.avgRowH() * n;
    this.keepingAnchor('collapseTop', () => this.above.update((c) => [...c, { count: n, height }]));
  }

  private collapseBottom(count: number): void {
    const n = Math.min(count, this.renderCount());
    if (n <= 0) return;
    const height = this.avgRowH() * n;
    this.keepingAnchor('collapseBottom', () => this.below.update((c) => [...c, { count: n, height }]));
  }

  // ---- viewport ------------------------------------------------------------

  /** Run a mutation, keeping the message at the top of the viewport in place.
   *  Public for fetches that grow the list from outside. */
  keepingAnchor(label: string, mutate: () => void): void {
    const anchor = this.topAnchor();
    this.withScrollLock(() => {
      mutate();
      this.tick();
      let reanchor = 0;
      if (anchor) {
        const el = this.msgEls().find((e) => e.dataset['id'] === anchor.id);
        if (el) {
          const now = el.getBoundingClientRect().top - this.host.getBoundingClientRect().top;
          reanchor = now - anchor.top;
          this.host.scrollTop += reanchor;
        }
      }
      if (this.dbg) {
        this.log(label, {
          reanchor: +reanchor.toFixed(1),
          renderCount: this.renderCount(),
          above: this.above().length,
          below: this.below().length,
        });
      }
    });
  }

  topAnchor(): { id: string; top: number } | null {
    const hostTop = this.host.getBoundingClientRect().top;
    for (const e of this.msgEls()) {
      const r = e.getBoundingClientRect();
      if (r.bottom >= hostTop) return { id: e.dataset['id'] ?? '', top: r.top - hostTop };
    }
    return null;
  }

  scrollToBottom(): void {
    this.following = true;
    this.host.scrollTop = this.host.scrollHeight;
  }

  scrollToTs(ts: number): void {
    // Landing mid-history is not following, whether or not `ts` is rendered.
    this.following = false;
    const head = this.host.querySelector<HTMLElement>('.thread-head')?.offsetHeight ?? 0;
    const hostTop = this.host.getBoundingClientRect().top;
    const target = this.msgEls().find((e) => Number(e.dataset['ts']) >= ts) ?? this.msgEls()[0];
    if (!target) return;
    this.host.scrollTop += target.getBoundingClientRect().top - hostTop - head;
  }

  /** Keep the conversation's end visible when the scroll container resizes, as
   *  when the soft keyboard opens. `interactive-widget=resizes-content` keeps the
   *  composer above the keys, but a resize leaves `scrollTop` alone and the newest
   *  messages under the keyboard. Any resize: re-pinning at the bottom is a no-op.
   *  Returns its teardown. */
  observeShrink(): () => void {
    // jsdom has no ResizeObserver; `repinAfterResize` is unit-tested, the
    // geometry browser-tested.
    if (typeof ResizeObserver === 'undefined') return () => undefined;
    const ro = new ResizeObserver(() => this.repinAfterResize());
    this.ro = ro;
    ro.observe(this.host);
    return () => {
      ro.disconnect();
      this.ro = null;
      this.watched = null;
    };
  }

  /** Watch the message block too: an image loading after the open has scrolled
   *  grows it and pushes the bottom away. A size observer, unlike load listeners,
   *  cannot miss an image that finished before it was attached. Re-pointed at
   *  each new block the caller offers. */
  watchContent(el: HTMLElement | undefined): void {
    const next = el ?? null;
    if (!this.ro || next === this.watched) return;
    if (this.watched) this.ro.unobserve(this.watched);
    this.watched = next;
    if (next) this.ro.observe(next);
  }

  /** Follow the end down to the new bottom, or leave a reader in history where
   *  they are. Returns whether it moved the viewport. */
  repinAfterResize(): boolean {
    if (!this.following) return false;
    this.withScrollLock(() => this.scrollToBottom());
    this.log('repinAfterResize', { clientHeight: this.host.clientHeight });
    return true;
  }

  /** Set `adjusting` for a programmatic scroll and the event it dispatches
   *  before the next frame. */
  withScrollLock(fn: () => void): void {
    this.adjusting = true;
    fn();
    requestAnimationFrame(() => (this.adjusting = false));
  }

  // ---- measuring -----------------------------------------------------------

  private msgEls(): HTMLElement[] {
    const el = this.container();
    return el ? Array.from(el.querySelectorAll<HTMLElement>('.msg')) : [];
  }

  private avgRowH(): number {
    const el = this.container();
    const n = this.renderCount();
    return el && n > 0 ? el.offsetHeight / n : ROW_GUESS;
  }

  private offscreenCounts(): { above: number; below: number } {
    const hostTop = this.host.getBoundingClientRect().top;
    const hostBottom = hostTop + this.host.clientHeight;
    let above = 0;
    let below = 0;
    for (const e of this.msgEls()) {
      const r = e.getBoundingClientRect();
      if (r.bottom < hostTop) above++;
      else if (r.top > hostBottom) below++;
    }
    return { above, below };
  }

  /** Log when visible content shifts on its own: the top message moved more than
   *  the scroll delta explains. */
  private detectJump(): void {
    const a = this.topAnchor();
    const top = this.host.scrollTop;
    if (a && this.lastAnchor?.id === a.id) {
      const expected = this.lastAnchor.top - (top - this.lastScrollTop);
      const shift = a.top - expected;
      if (Math.abs(shift) > 1) {
        this.log('jump', { shift: +shift.toFixed(1), anchor: a.id, renderCount: this.renderCount() });
      }
    }
  }

  private log(op: string, data: Record<string, unknown>): void {
    if (this.dbg) console.debug('[thread-scroll]', op, data);
  }
}
