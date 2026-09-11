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
/** How many pages `loadThread` will fetch to restore a saved scroll depth.
 *
 *  Five times `MAX_RENDERED`, so a legitimate restore never reaches it: what it
 *  bounds is a `?from` that no longer sits anywhere near the newest page — a
 *  stale bookmark, or a hand-edited URL — where the loop would otherwise issue
 *  one request per hundred messages all the way back. See `loadThread`. */
export const MAX_RESTORE_PAGES = (MAX_RENDERED * 5) / PAGE;
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

  /** Whether the reader is following the END of the conversation rather than
   *  reading back inside it.
   *
   *  ⚠ **Maintained as state because the moment it is needed it can no longer be
   *  measured.** `repinAfterResize` runs after the container has already shrunk,
   *  and by then the bottom has moved away on its own — asking `atBottom()` there
   *  answers for the new geometry, which is the question nobody asked. So every
   *  scroll and every programmatic move records it instead.
   *
   *  Starts true: a conversation opens at the latest message. */
  private following = true;

  /** The container height the last scroll event was seen at, so a scroll the
   *  RESIZE caused can be told from one the reader caused. See `step`. */
  private hostHeight = 0;
  /** And the content height, for the same reason in the other direction — see
   *  `noteScroll`: growth BELOW a reader who is at the end moves the bottom away
   *  from them without their having moved. */
  private hostScrollHeight = 0;

  /** The observer behind `observeShrink`, and the message block it is currently
   *  pointed at — the host never changes, that element does. */
  private ro: ResizeObserver | null = null;
  private watched: HTMLElement | null = null;

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
    this.following = true; // a fresh conversation opens at its latest message
  }

  /** Advance the window for wherever the scroll is now.
   *
   *  Returns whether the top edge is close AND nothing is left collapsed above
   *  it — which is the caller's cue to fetch an older page, the one thing here
   *  that needs to know where messages come from. */
  step(): { needOlder: boolean; needNewer: boolean } {
    const el = this.container();
    if (!el) return { needOlder: false, needNewer: false };
    if (this.dbg) this.detectJump();

    // Proximity to the message block's edges, measured from viewport rects (the
    // host's offsetParent isn't guaranteed to be the host, so offsetTop is not).
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
    // ⚠ **THE MIRROR OF `needOlder`, and it did not exist until #1401.** The
    // window could only ever grow backwards, because the only route by which
    // newer messages reached the thread was `pollNewer` asking for the NEWEST
    // page. That is fine for a window anchored to the present and useless for
    // one that is not: a reader landed on a 2005 search hit could scroll back
    // for ever and not forward one line.
    //
    // Same shape as the top: reveal what is collapsed below if there is any,
    // and otherwise say that something has to be fetched. Who fetches it is not
    // this engine's business — it does not know where messages come from.
    if (nearBottom) {
      if (this.below().length) {
        this.revealBottom();
        this.enforceMax('top');
      } else {
        needNewer = true;
      }
    }
    // Re-baseline after any windowing so the next jump check compares like
    // frames (a windowing step legitimately re-anchors; that isn't a jump).
    if (this.dbg) {
      this.lastAnchor = this.topAnchor();
      this.lastScrollTop = this.host.scrollTop;
    }
    return { needOlder, needNewer };
  }

  /** ⚠ **EVERY scroll event updates `following`, INCLUDING the ones the window
   *  itself caused — which is why this is not part of `step`.**
   *
   *  `onScroll` skips `step` whenever the window is busy or a load is in flight,
   *  and for the windowing that is right: a programmatic scroll is not the reader
   *  looking around. But `following` is not about WHO scrolled, it is about where
   *  the viewport ended up, and leaving it un-updated through those stretches
   *  leaves it stale-TRUE exactly when the reader has gone back into history —
   *  so the next resize pins them to the present instead. Measured 2026-09-10 on
   *  a variant that observed content growth as well: `routing.spec.ts`'s
   *  "scrolling to the top auto-loads older messages" timed out at 90s in 6 runs
   *  of 20, each arriving page of history yanking the viewport back.
   *
   *  Reading the position during our own scroll is the point rather than a
   *  hazard: after `scrollToBottom` it answers true, after `scrollToTs` or a
   *  re-anchored prepend it answers false, and all three are the truth.
   *
   *  ⚠ A SHRINKING CONTAINER CAN DISPATCH A SCROLL EVENT OF ITS OWN, and that one
   *  must not read as the reader leaving the end: the bottom moved, they did not.
   *  `scrollTop` is identical either way, so the height it was last seen at is
   *  what separates them. */
  noteScroll(): void {
    const h = this.host.clientHeight;
    const sh = this.host.scrollHeight;
    const grew = Math.max(0, sh - this.hostScrollHeight);
    const resized = h !== this.hostHeight;
    this.hostHeight = h;
    this.hostScrollHeight = sh;
    if (resized) return;
    // ⚠ **GROWTH BELOW A READER AT THE END IS NOT THE READER LEAVING IT.** An
    // image loading under the newest message pushes the bottom away, `atBottom`
    // answers false for a moment, and a scroll event landing in that moment used
    // to record them as having wandered off — after which the observer politely
    // declined to re-pin. Measured 2026-09-11 against the deterministic harness:
    // 1 run in 4 still landed 585px short with the observer in place, and this
    // is why.
    //
    // The test is whether the whole gap is explained by what just grew. A reader
    // who actually scrolled away is further off than the growth accounts for; one
    // standing still is exactly that far and no further.
    if (this.following && grew > 0 && sh - this.host.scrollTop - h <= grew + BOTTOM_EPS) return;
    this.following = this.atBottom();
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
    this.following = true;
    this.host.scrollTop = this.host.scrollHeight;
  }

  scrollToTs(ts: number): void {
    // Landing mid-history is the definition of not following the end, whether or
    // not a row for `ts` turns out to be rendered — so it is recorded before the
    // early return below, not after it.
    this.following = false;
    const head = this.host.querySelector<HTMLElement>('.thread-head')?.offsetHeight ?? 0;
    const hostTop = this.host.getBoundingClientRect().top;
    const target = this.msgEls().find((e) => Number(e.dataset['ts']) >= ts) ?? this.msgEls()[0];
    if (!target) return;
    this.host.scrollTop += target.getBoundingClientRect().top - hostTop - head;
  }

  /** Keep the bottom of the conversation visible when the SCROLL CONTAINER
   *  shrinks under it — on a phone, the soft keyboard opening the moment the
   *  reader starts typing a reply.
   *
   *  ⚠ **`interactive-widget=resizes-content` is only half of the job.** That
   *  token (index.html) makes the Android keyboard shrink the layout viewport
   *  instead of sliding over the page, and what it buys is the COMPOSER staying
   *  above the keys. It does nothing for the conversation: a resize leaves
   *  `scrollTop` exactly where it was, so the newest messages go below the fold
   *  by the keyboard's full height. Measured 2026-09-10 at 412x839 with a 350px
   *  keyboard: the thread sat 350px from the bottom, the last message rendered at
   *  y 732 with the fold at 489, and the reader was typing a reply to messages
   *  they could no longer see. The two browser tests that guard the keyboard both
   *  passed — they assert where the composer is, and neither looks at a message.
   *
   *  Any resize, not just a shrink: the keyboard closing again, a rotation, a
   *  desktop pane being dragged. Re-pinning when already at the bottom is a
   *  no-op, so the cheap condition is the right one.
   *
   *  Returns its own teardown. */
  observeShrink(): () => void {
    // jsdom has neither a ResizeObserver nor the layout to feed one. The unit
    // suite covers the decision (`repinAfterResize`), the browser suite the
    // geometry — same split as the rest of this file.
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

  /** Also watch the MESSAGE BLOCK, whose growth is the other way the bottom moves
   *  away from a reader who was at it: a lazily-loaded image finishing after the
   *  open has already scrolled, with no reserved height standing in for it.
   *
   *  ⚠ **This is what the listener loop it replaces could not do.** That loop
   *  (deleted with this change) attached `load` only to images not yet
   *  `complete`, so one finishing
   *  between the scroll and the loop gets no listener and its growth is never
   *  compensated. Proved by perturbation 2026-09-11: delay the loop by 150ms, so
   *  every image is complete before it runs, and the open lands **780px** short,
   *  4 runs of 4 — all four images. The gate's own flake is the same bug with
   *  whichever subset happened to win the race, which is why it measured 271px
   *  and only about one run in five.
   *
   *  A box either changed size or it did not, so there is no window to fall into.
   *
   *  The element is new after every render that recreates it, so the caller
   *  re-offers the current one and this re-points the observer. */
  watchContent(el: HTMLElement | undefined): void {
    const next = el ?? null;
    if (!this.ro || next === this.watched) return;
    if (this.watched) this.ro.unobserve(this.watched);
    this.watched = next;
    if (next) this.ro.observe(next);
  }

  /** The decision half of `observeShrink`: follow the end of the conversation
   *  down to the new bottom, or leave a reader who is back in history exactly
   *  where they are. Returns whether it moved the viewport. */
  repinAfterResize(): boolean {
    if (!this.following) return false;
    this.withScrollLock(() => this.scrollToBottom());
    this.log('repinAfterResize', { clientHeight: this.host.clientHeight });
    return true;
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
