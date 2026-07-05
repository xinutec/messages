import { ApplicationRef, Component, ElementRef, computed, effect, inject, input, signal, viewChild } from '@angular/core';
import { DatePipe } from '@angular/common';
import { ActivatedRoute, Router } from '@angular/router';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';

import { firstValueFrom } from 'rxjs';

import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
import { Conversation, Message, Origin } from './models';

/** Server page size (also our collapse/reveal chunk size — one fetched page is
 *  one windowing unit). */
const PAGE = 100;
/** Soft cap on how many messages are ever in the DOM. The rest live only as
 *  retained JS data (cheap — text) behind measured spacer divs, so scrolling far
 *  back and navigating away stay cheap. */
const MAX_RENDERED = 400;
/** How close (px) to an edge before we load/reveal — big enough to stay ahead of
 *  the scroll so the user rarely sees the blank spacer. */
const EDGE = 1200;
/** First guess for a row's height, replaced by real measurements once rendered.
 *  Only affects spacer sizing (scrollbar geometry); the viewport is kept correct
 *  by anchoring, not by these estimates. */
const ROW_GUESS = 64;
/** How close (px) to the very bottom counts as "at the latest", where we clear
 *  ?from so a refresh reopens pinned to the bottom. Small (unlike EDGE) so a
 *  short thread isn't treated as permanently at the bottom. */
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

@Component({
  selector: 'app-thread',
  templateUrl: './thread.html',
  styleUrl: './thread.scss',
  // The host IS the scroll container (class `thread`): the sticky head + day
  // headers pin against it, and it's where we read/adjust scrollTop.
  host: { class: 'thread', '(scroll)': 'onScroll()' },
  imports: [DatePipe, MatButtonModule, MatIconModule, MatProgressBarModule],
})
export class Thread {
  private api = inject(MessagesApi);
  private store = inject(MessagesStore);
  private router = inject(Router);
  private route = inject(ActivatedRoute);
  private appRef = inject(ApplicationRef);
  private host = inject<ElementRef<HTMLElement>>(ElementRef).nativeElement;
  private readonly messagesEl = viewChild<ElementRef<HTMLElement>>('messagesEl');

  // Bound from the route (withComponentInputBinding); both absent on `/` → the
  // placeholder. Ids can contain ':' and '/'; the router encodes/decodes them.
  // Our navigation only ever routes valid origins, so `origin` is typed as such.
  readonly origin = input<Origin>();
  readonly id = input<string>();

  // A conversation is routed (vs the '' placeholder route) when both params are
  // bound. Kept as a boolean so the template doesn't compare signals to null.
  readonly routed = computed(() => this.origin() != null && this.id() != null);

  readonly conversation = computed<Conversation | null>(() => {
    const o = this.origin();
    const i = this.id();
    return o != null && i != null ? this.store.find(o, i) : null;
  });

  // Title from the loaded list when available; a deep link can render the thread
  // before the list arrives, so fall back rather than block.
  readonly headTitle = computed(() => {
    const c = this.conversation();
    return c ? this.store.title(c) : 'Conversation';
  });

  /** All fetched messages, ascending by ts. Retained in full (text is cheap);
   *  only a window of them is ever rendered. */
  readonly messages = signal<Message[]>([]);
  /** Older messages collapsed above the window; newer ones collapsed below.
   *  `above[last]`/`below[last]` are the chunks nearest the rendered window, so
   *  reveal pops the end. */
  private readonly above = signal<Chunk[]>([]);
  private readonly below = signal<Chunk[]>([]);

  private readonly lo = computed(() => sumCount(this.above()));
  private readonly end = computed(() => this.messages().length - sumCount(this.below()));
  /** The messages currently in the DOM (a window over `messages`). */
  readonly rendered = computed(() => this.messages().slice(this.lo(), this.end()));
  readonly renderCount = computed(() => this.end() - this.lo());
  readonly topSpacer = computed(() => sumHeight(this.above()));
  readonly bottomSpacer = computed(() => sumHeight(this.below()));

  readonly loadingThread = signal(false);
  readonly loadingOlder = signal(false);
  readonly hasMore = signal(false);
  readonly threadError = signal(false);
  private cursor: string | null = null;

  // Suppress our own scroll handler while we programmatically adjust scrollTop.
  private adjusting = false;
  private fromTimer: ReturnType<typeof setTimeout> | null = null;

  // While the user is pinned to the newest message (a fresh open at the bottom),
  // images in the last messages load lazily and grow the layout, which would push
  // the latest messages below the fold. Re-pin to the bottom as they settle — a
  // bumped token cancels pending re-pins the moment the user scrolls away.
  private pinToken = 0;

  // Optional scroll-jump instrumentation (off by default). Enable at runtime with
  // `localStorage.threadScrollDebug = '1'` or a `?scrolldebug` URL param, then
  // read the `[thread-scroll]` console.debug lines: a `jump` line = already-
  // visible content shifted on its own (the symptom); an op line (`revealTop`,
  // `fetchOlder`, …) shows how far that step had to re-anchor the viewport.
  private dbg = false;
  private lastAnchor: { id: string; top: number } | null = null;
  private lastScrollTop = 0;

  /** Rendered messages bucketed by calendar day, so each day renders as a section
   *  with a sticky date header (the header pins only within its own day → it
   *  shows the current top message's date and is replaced by the next day, never
   *  stacks). */
  readonly dayGroups = computed(() => {
    const groups: { key: string; ts: number; items: Message[] }[] = [];
    let lastKey: string | null = null;
    for (const m of this.rendered()) {
      const key = new Date(m.ts).toDateString();
      if (key === lastKey) {
        groups[groups.length - 1].items.push(m);
      } else {
        groups.push({ key, ts: m.ts, items: [m] });
        lastKey = key;
      }
    }
    return groups;
  });

  constructor() {
    this.dbg = readDebugFlag();
    // (Re)load whenever the routed conversation changes — deep link, switching
    // conversations (Angular reuses this instance, just updates the inputs),
    // Back/forward.
    let loadedKey: string | null = null;
    effect(() => {
      const o = this.origin();
      const i = this.id();
      const key = o != null && i != null ? `${o}:${i}` : null;
      if (key === loadedKey) return;
      loadedKey = key;
      if (o != null && i != null) void this.loadThread(o, i);
      else this.resetState();
    });
  }

  private resetState(): void {
    this.messages.set([]);
    this.above.set([]);
    this.below.set([]);
    this.hasMore.set(false);
    this.cursor = null;
  }

  /** Load the thread. Restores the paged-back depth from ?from (the ts the user
   *  was looking at) so a refresh/deep-link returns to the same spot; otherwise
   *  opens pinned to the latest message, like a chat app. */
  private async loadThread(origin: Origin, id: string): Promise<void> {
    this.resetState();
    this.threadError.set(false);
    this.loadingThread.set(true);
    const from = Number(this.route.snapshot.queryParamMap.get('from')) || null;
    try {
      const first = await firstValueFrom(this.api.messages(origin, id, undefined, PAGE));
      let msgs = first.messages;
      let hasMore = first.has_more;
      let cursor = first.next_cursor;
      // Page older until we've reached the saved depth (or run out).
      while (from != null && hasMore && cursor != null && msgs.length > 0 && msgs[0].ts > from) {
        const older = await firstValueFrom(this.api.messages(origin, id, cursor, PAGE));
        if (older.messages.length === 0) break;
        msgs = [...older.messages, ...msgs];
        hasMore = older.has_more;
        cursor = older.next_cursor;
      }
      this.messages.set(msgs);
      this.hasMore.set(hasMore);
      this.cursor = cursor;
      this.loadingThread.set(false);
      this.appRef.tick();
      // Position the viewport, then bound the DOM around it.
      this.withScrollLock(() => {
        if (from != null) this.scrollToTs(from);
        else this.scrollToBottom();
      });
      this.trimToWindow();
      // Opened at the latest message: hold the bottom as lazy images load in.
      if (from == null) this.keepPinnedToBottom();
    } catch {
      this.threadError.set(true);
      this.loadingThread.set(false);
    }
  }

  reload(): void {
    const o = this.origin();
    const i = this.id();
    if (o != null && i != null) void this.loadThread(o, i);
  }

  /** In-app back (the mobile single-pane control) = return to the list route,
   *  keeping the origin filter and dropping the paged depth. */
  back(): void {
    void this.router.navigate(['/'], { queryParams: { from: null }, queryParamsHandling: 'merge' });
  }

  // ---- scroll-driven windowing -------------------------------------------

  onScroll(): void {
    if (this.adjusting || this.loadingThread() || this.threadError() || !this.routed()) return;
    // A genuine user scroll means "I'm looking around" — stop auto-pinning to the
    // bottom (a bumped token cancels any pending image-load re-pin).
    this.pinToken++;
    const el = this.messagesEl()?.nativeElement;
    if (!el) return;
    if (this.dbg) this.detectJump();
    // Proximity to the message block's edges, measured from viewport rects (the
    // host's offsetParent isn't guaranteed to be the host, so offsetTop is not).
    const hostRect = this.host.getBoundingClientRect();
    const elRect = el.getBoundingClientRect();
    const nearTop = elRect.top - hostRect.top >= -EDGE;
    const nearBottom = elRect.bottom - hostRect.bottom <= EDGE;

    if (nearTop) {
      if (this.above().length) {
        this.revealTop();
        this.enforceMax('bottom');
      } else {
        this.fetchOlder();
      }
    }
    if (nearBottom && this.below().length) {
      this.revealBottom();
      this.enforceMax('top');
    }
    // Re-baseline after any windowing so the next jump check compares like frames
    // (a windowing step legitimately re-anchors; that isn't a jump).
    if (this.dbg) {
      this.lastAnchor = this.topAnchor();
      this.lastScrollTop = this.host.scrollTop;
    }
    this.scheduleFromParam();
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

  private fetchOlder(): void {
    const o = this.origin();
    const i = this.id();
    if (o == null || i == null || !this.hasMore() || this.cursor == null || this.loadingOlder()) return;
    this.loadingOlder.set(true);
    this.api.messages(o, i, this.cursor, PAGE).subscribe({
      next: (page) => {
        // Prepend; the window starts at index 0 (above is empty when we fetch),
        // so the new page becomes rendered at the top. Anchor keeps the viewport
        // on the same message despite the added height.
        this.mutateWithAnchor('fetchOlder', () => {
          this.messages.update((cur) => [...page.messages, ...cur]);
          this.hasMore.set(page.has_more);
          this.cursor = page.next_cursor;
          this.loadingOlder.set(false);
        });
        this.enforceMax('bottom');
        this.scheduleFromParam();
      },
      error: () => this.loadingOlder.set(false),
    });
  }

  private revealTop(): void {
    const cs = this.above();
    if (!cs.length) return;
    this.mutateWithAnchor('revealTop', () => this.above.set(cs.slice(0, -1)));
  }

  private revealBottom(): void {
    const cs = this.below();
    if (!cs.length) return;
    this.mutateWithAnchor('revealBottom', () => this.below.set(cs.slice(0, -1)));
  }

  private collapseTop(count: number): void {
    const n = Math.min(count, this.renderCount());
    if (n <= 0) return;
    const height = this.avgRowH() * n;
    this.mutateWithAnchor('collapseTop', () => this.above.update((c) => [...c, { count: n, height }]));
  }

  private collapseBottom(count: number): void {
    const n = Math.min(count, this.renderCount());
    if (n <= 0) return;
    const height = this.avgRowH() * n;
    this.mutateWithAnchor('collapseBottom', () => this.below.update((c) => [...c, { count: n, height }]));
  }

  /** Keep the DOM bounded by collapsing the end away from the viewport. Called
   *  after growing the opposite end, so the collapsed rows are off-screen. */
  private enforceMax(side: 'top' | 'bottom'): void {
    let guard = 0;
    while (this.renderCount() > MAX_RENDERED && guard++ < 64) {
      if (side === 'bottom') this.collapseBottom(PAGE);
      else this.collapseTop(PAGE);
    }
  }

  /** After a deep-link restore we may have rendered many pages; collapse the ends
   *  that are off-screen until the DOM is back under the cap. */
  private trimToWindow(): void {
    let guard = 0;
    while (this.renderCount() > MAX_RENDERED && guard++ < 128) {
      const { above, below } = this.offscreenCounts();
      if (below >= above && below > 0) this.collapseBottom(Math.min(PAGE, below));
      else if (above > 0) this.collapseTop(Math.min(PAGE, above));
      else break; // nothing off-screen to collapse (viewport bigger than cap)
    }
  }

  // ---- DOM helpers --------------------------------------------------------

  private msgEls(): HTMLElement[] {
    const el = this.messagesEl()?.nativeElement;
    return el ? Array.from(el.querySelectorAll<HTMLElement>('.msg')) : [];
  }

  private avgRowH(): number {
    const el = this.messagesEl()?.nativeElement;
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

  /** Run a window mutation while keeping the viewport pinned to whatever message
   *  is at the top of it — the estimated spacer heights don't have to be exact
   *  because the anchor, not the geometry, decides what the user sees. */
  private mutateWithAnchor(label: string, mutate: () => void): void {
    const anchor = this.topAnchor();
    this.withScrollLock(() => {
      mutate();
      this.appRef.tick();
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

  private topAnchor(): { id: string; top: number } | null {
    const hostTop = this.host.getBoundingClientRect().top;
    for (const e of this.msgEls()) {
      const r = e.getBoundingClientRect();
      if (r.bottom >= hostTop) return { id: e.dataset['id'] ?? '', top: r.top - hostTop };
    }
    return null;
  }

  private scrollToBottom(): void {
    this.host.scrollTop = this.host.scrollHeight;
  }

  /** Keep the viewport pinned to the bottom as not-yet-loaded images in the
   *  rendered window finish and grow the layout. One-shot per open; each pending
   *  image re-pins on load, and the whole session is superseded when the user
   *  scrolls (onScroll bumps pinToken) or the thread reloads (loadThread does). */
  private keepPinnedToBottom(): void {
    const token = ++this.pinToken;
    const el = this.messagesEl()?.nativeElement;
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

  private scrollToTs(ts: number): void {
    const head = this.host.querySelector<HTMLElement>('.thread-head')?.offsetHeight ?? 0;
    const hostTop = this.host.getBoundingClientRect().top;
    const target = this.msgEls().find((e) => Number(e.dataset['ts']) >= ts) ?? this.msgEls()[0];
    if (!target) return;
    this.host.scrollTop += target.getBoundingClientRect().top - hostTop - head;
  }

  /** Set adjusting for the duration of a programmatic scroll change AND the
   *  scroll event it triggers (dispatched before the next frame). */
  private withScrollLock(fn: () => void): void {
    this.adjusting = true;
    fn();
    requestAnimationFrame(() => (this.adjusting = false));
  }

  // ---- ?from (scroll-position restore) -----------------------------------

  private scheduleFromParam(): void {
    if (this.fromTimer) clearTimeout(this.fromTimer);
    this.fromTimer = setTimeout(() => this.commitFromParam(), 300);
  }

  private commitFromParam(): void {
    const o = this.origin();
    const i = this.id();
    if (o == null || i == null) return;
    const atBottom =
      this.below().length === 0 &&
      this.host.scrollHeight - this.host.scrollTop - this.host.clientHeight <= BOTTOM_EPS;
    const from = atBottom ? null : this.topAnchor()?.id;
    const ts = from ? this.messages().find((m) => m.id === from)?.ts : null;
    void this.router.navigate([], {
      relativeTo: this.route,
      queryParams: { from: ts != null ? String(ts) : null },
      queryParamsHandling: 'merge',
      replaceUrl: true,
    });
  }
}
