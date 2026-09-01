// Keeping a long conversation's DOM bounded while it still scrolls like one
// continuous thread.
//
// The thread retains every message it has fetched — text is cheap — but renders
// only a window of them, standing spacer divs in for the runs collapsed off
// either end so the scrollbar keeps roughly the right shape. Scrolling towards
// an edge reveals the nearest collapsed run, or asks the caller to fetch.
//
// ⚠ **The estimates do not have to be right.** Spacer heights are averages, so
// the scrollbar's geometry is approximate on purpose. What the user sees is kept
// exact by a different mechanism: every mutation re-anchors the viewport on the
// message that was at the top of it. Correcting the estimates would not remove
// the need for the anchor, and the anchor removes the need for good estimates.
//
// Split out of `thread.ts` because none of it knows what a conversation is: it
// takes an element, a list and a tick, and it is the part of that component with
// invariants subtle enough to be worth reading on their own.

import { Signal, computed, signal } from '@angular/core';

import { Message } from './models';

/** Server page size, and so the unit a run is collapsed or revealed in. */
export const PAGE = 100;
/** Soft cap on how many messages are ever in the DOM. */
export const MAX_RENDERED = 400;
/** How close (px) to an edge before we load/reveal — big enough to stay ahead of
 *  the scroll so the user rarely sees the blank spacer. */
const EDGE = 1200;
/** First guess for a row's height, replaced by real measurements once rendered.
 *  Only affects spacer sizing (scrollbar geometry); the viewport is kept correct
 *  by anchoring, not by these estimates. */
const ROW_GUESS = 64;
/** How close (px) to the very bottom counts as "at the latest". Small (unlike
 *  EDGE) so a short thread isn't treated as permanently at the bottom. */
const BOTTOM_EPS = 64;

/** A run of messages collapsed out of the DOM: how many, and the pixel height
 *  they occupied (so the spacer standing in for them is ~the right size). */
interface Chunk {
  count: number;
  height: number;
}

const sumCount = (cs: Chunk[]): number => cs.reduce((a, c) => a + c.count, 0);
const sumHeight = (cs: Chunk[]): number => cs.reduce((a, c) => a + c.height, 0);

/** Whether to emit scroll-jump diagnostics. Off unless explicitly enabled, and
 *  guarded because localStorage/location can throw (SSR, sandboxed iframes). */
function readDebugFlag(): boolean {
  try {
    if (/(?:^|[?&])scrolldebug\b/.test(location.search)) return true;
    return localStorage.getItem('threadScrollDebug') === '1';
  } catch {
    return false;
  }
}

export class ThreadWindow {
  /** Older messages collapsed above the window; newer ones collapsed below.
   *  `above[last]`/`below[last]` are the chunks nearest the rendered window, so
   *  reveal pops the end. */
  private readonly above = signal<Chunk[]>([]);
  private readonly below = signal<Chunk[]>([]);

  private readonly lo = computed(() => sumCount(this.above()));
  private readonly end = computed(() => this.messages().length - sumCount(this.below()));

  /** The messages currently in the DOM (a window over the retained list). */
  readonly rendered = computed(() => this.messages().slice(this.lo(), this.end()));
  readonly renderCount = computed(() => this.end() - this.lo());
  readonly topSpacer = computed(() => sumHeight(this.above()));
  readonly bottomSpacer = computed(() => sumHeight(this.below()));

  /** Set while we move scrollTop ourselves, so the caller's scroll handler can
   *  tell a programmatic adjustment from a user's scroll. */
  private adjusting = false;
  get busy(): boolean {
    return this.adjusting;
  }

  /** Bumped by any real user scroll and by each new open, cancelling a pending
   *  re-pin from an earlier one. */
  private pinToken = 0;

  // Optional scroll-jump instrumentation (off by default). Enable at runtime
  // with `localStorage.threadScrollDebug = '1'` or a `?scrolldebug` URL param,
  // then read the `[thread-scroll]` console.debug lines: a `jump` line =
  // already-visible content shifted on its own (the symptom); an op line
  // (`revealTop`, `fetchOlder`, …) shows how far that step re-anchored.
  private readonly dbg = readDebugFlag();
  private lastAnchor: { id: string; top: number } | null = null;
  private lastScrollTop = 0;

  constructor(
    private readonly host: HTMLElement,
    /** The message container, once the view has it — a getter because a
     *  `viewChild` resolves after construction and again after each render. */
    private readonly container: () => HTMLElement | undefined,
    private readonly messages: Signal<Message[]>,
    /** Flush pending renders, so the DOM can be measured. */
    private readonly tick: () => void,
  ) {}

  /** Back to "nothing collapsed" — for a fresh conversation. */
  reset(): void {
    this.above.set([]);
    this.below.set([]);
  }

  /** Advance the window for wherever the scroll is now.
   *
   *  Returns whether the top edge is close AND nothing is left collapsed above
   *  it — which is the caller's cue to fetch an older page, the one thing here
   *  that needs to know where messages come from. */
  step(): { needOlder: boolean } {
    // A genuine user scroll means "I'm looking around" — stop auto-pinning to
    // the bottom.
    this.pinToken++;
    const el = this.container();
    if (!el) return { needOlder: false };
    if (this.dbg) this.detectJump();

    // Proximity to the message block's edges, measured from viewport rects (the
    // host's offsetParent isn't guaranteed to be the host, so offsetTop is not).
    const hostRect = this.host.getBoundingClientRect();
    const elRect = el.getBoundingClientRect();
    const nearTop = elRect.top - hostRect.top >= -EDGE;
    const nearBottom = elRect.bottom - hostRect.bottom <= EDGE;

    let needOlder = false;
    if (nearTop) {
      if (this.above().length) {
        this.revealTop();
        this.enforceMax('bottom');
      } else {
        needOlder = true;
      }
    }
    if (nearBottom && this.below().length) {
      this.revealBottom();
      this.enforceMax('top');
    }
    // Re-baseline after any windowing so the next jump check compares like
    // frames (a windowing step legitimately re-anchors; that isn't a jump).
    if (this.dbg) {
      this.lastAnchor = this.topAnchor();
      this.lastScrollTop = this.host.scrollTop;
    }
    return { needOlder };
  }

  /** At the end of the conversation: nothing collapsed below, and within a few
   *  pixels of the bottom. */
  atBottom(): boolean {
    return (
      this.below().length === 0 &&
      this.host.scrollHeight - this.host.scrollTop - this.host.clientHeight <= BOTTOM_EPS
    );
  }

  /** Keep the DOM bounded by collapsing the end away from the viewport. Call
   *  after growing the opposite end, so the collapsed rows are off-screen. */
  enforceMax(side: 'top' | 'bottom'): void {
    let guard = 0;
    while (this.renderCount() > MAX_RENDERED && guard++ < 64) {
      if (side === 'bottom') this.collapseBottom(PAGE);
      else this.collapseTop(PAGE);
    }
  }

  /** After a deep-link restore we may have rendered many pages; collapse the
   *  ends that are off-screen until the DOM is back under the cap. */
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

  /** Run a mutation while keeping the viewport pinned to whatever message is at
   *  the top of it — this is what makes the estimated spacer heights good
   *  enough. Public because fetching an older page grows the list from outside
   *  here and must not move what the user is reading. */
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
    this.host.scrollTop = this.host.scrollHeight;
  }

  scrollToTs(ts: number): void {
    const head = this.host.querySelector<HTMLElement>('.thread-head')?.offsetHeight ?? 0;
    const hostTop = this.host.getBoundingClientRect().top;
    const target = this.msgEls().find((e) => Number(e.dataset['ts']) >= ts) ?? this.msgEls()[0];
    if (!target) return;
    this.host.scrollTop += target.getBoundingClientRect().top - hostTop - head;
  }

  /** Keep the viewport pinned to the bottom as not-yet-loaded images in the
   *  rendered window finish and grow the layout. One-shot per open; each pending
   *  image re-pins on load, and the whole session is superseded when the user
   *  scrolls (`step` bumps pinToken) or the thread reloads. */
  keepPinnedToBottom(): void {
    const token = ++this.pinToken;
    const el = this.container();
    if (!el) return;
    for (const img of Array.from(el.querySelectorAll('img'))) {
      if (img.complete) continue;
      const onSettle = (): void => {
        if (token !== this.pinToken) return; // user scrolled away, or a newer open won
        this.withScrollLock(() => this.scrollToBottom());
      };
      img.addEventListener('load', onSettle, { once: true });
      img.addEventListener('error', onSettle, { once: true });
    }
  }

  /** Set `adjusting` for the duration of a programmatic scroll change AND the
   *  scroll event it triggers (dispatched before the next frame). */
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

  /** Log when already-visible content shifts on its own — i.e. between two user
   *  scroll frames the top message moved by more than the scroll delta explains.
   *  That residual IS the visible jump. */
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
