import { ApplicationRef, Component, DestroyRef, ElementRef, LOCALE_ID, computed, effect, inject, input, signal, viewChild } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { DatePipe, NgTemplateOutlet } from '@angular/common';
import { ActivatedRoute, Router } from '@angular/router';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatListModule } from '@angular/material/list';
import { MatMenuModule } from '@angular/material/menu';
import { MatCalendar } from '@angular/material/datepicker';
import { FormsModule } from '@angular/forms';
import { Pictures, ScaffoldActions, ScaffoldLeading, scaffoldTitle } from '@xinutec/ui-scaffold';

import { firstValueFrom } from 'rxjs';

import { attachmentName, attachmentNoun, isAudio, isImage, isVideo } from './attachment';
import { Avatar } from './avatar';
import { FetchRequests } from './fetch-requests';
import { segments } from './formatting';
import { hue } from './list-row';
import { chatLogHtml, formatChatLog } from './copy-log';
import { selectedMessages } from './selection';
import {
  attachmentUrl,
  dayGroups,
  deliveryLabel,
  deliveryPending,
  hostOf,
  isPlain,
  mib,
  reactors,
  readers,
  runStarts,
} from './message-view';
import { MAX_RESTORE_PAGES, PAGE, ThreadWindow } from './thread-window';
import { ThreadSearch } from './thread-search';
import { Composer } from './composer';
import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
import { Attachment, Conversation, Delivery, Message, Origin, ReplyTo, SearchHit } from './models';

/** How often an open, visible thread checks for newer messages. */
const POLL_MS = 5000;

@Component({
  selector: 'app-thread',
  templateUrl: './thread.html',
  styleUrls: ['./thread.scss', './thread-media.scss', './thread-message.scss'],
  // The host is the scroll container the sticky headers pin against. `copy`
  // bubbles here from wherever the selection is.
  host: {
    class: 'thread',
    '(scroll)': 'onScroll()',
    '(copy)': 'onCopy($event)',
    // The reader's own hand on the thread; see `readerMoved`.
    '(wheel)': 'readerMoved = true',
    '(touchmove)': 'readerMoved = true',
    '(pointerdown)': 'readerMoved = true',
    '(keydown)': 'readerMoved = true',
  },
  imports: [
    NgTemplateOutlet,
    DatePipe,
    FormsModule,
    MatButtonModule,
    MatIconModule,
    MatListModule,
    MatProgressBarModule,
    ScaffoldActions,
    ScaffoldLeading,
    Avatar,
    MatMenuModule,
    MatCalendar,
  ],
})
export class Thread {
  private api = inject(MessagesApi);
  private store = inject(MessagesStore);
  private router = inject(Router);
  private route = inject(ActivatedRoute);
  private appRef = inject(ApplicationRef);
  private locale = inject(LOCALE_ID);
  private host = inject<ElementRef<HTMLElement>>(ElementRef).nativeElement;
  private readonly messagesEl = viewChild<ElementRef<HTMLElement>>('messagesEl');

  // Shared with the clipboard; see attachment.ts.
  protected readonly attachmentName = attachmentName;

  private readonly pictures = inject(Pictures);

  /** A picture, full screen in the fleet's viewer; back closes it. */
  protected viewPicture(a: Attachment): void {
    this.pictures.open({ label: attachmentName(a) ?? 'picture', source: this.attachmentUrl(a) });
  }

  /** Where an attachment's bytes come from; see message-view.ts. */
  protected attachmentUrl(a: Attachment): string {
    const origin = this.origin();
    // Unreachable while routed; an empty src fails visibly.
    return origin ? attachmentUrl(origin, a) : '';
  }
  protected readonly attachmentNoun = attachmentNoun;

  /** A sender's hue, from `sender_key`: a rename keeps the colour. */
  protected senderHue(m: Message): number {
    return hue(m.sender_key);
  }

  protected readonly isImage = isImage;
  protected readonly isVideo = isVideo;
  protected readonly isAudio = isAudio;

  /** Deleted messages the reader chose to see, by id. Screen only (the clipboard
   *  still says `(deleted)`), reset with the conversation. Ids rather than a flag
   *  on the message, which `pollNewer` would replace. */
  private readonly revealedIds = signal<ReadonlySet<string>>(new Set());

  protected isRevealed(id: string): boolean {
    return this.revealedIds().has(id);
  }

  /** Messages showing their earlier versions, by id, like `revealedIds`. */
  private readonly openHistories = signal<ReadonlySet<string>>(new Set());

  protected isHistoryOpen(id: string): boolean {
    return this.openHistories().has(id);
  }

  protected toggleHistory(id: string): void {
    this.openHistories.update((cur) => toggled(cur, id));
  }

  protected toggleReveal(id: string): void {
    this.revealedIds.update((cur) => toggled(cur, id));
  }

  // Bound from the route; both absent on `/`. The router encodes ids containing
  // ':' and '/'.
  readonly origin = input<Origin>();
  readonly id = input<string>();

  readonly routed = computed(() => this.origin() != null && this.id() != null);

  readonly conversation = computed<Conversation | null>(() => {
    const o = this.origin();
    const i = this.id();
    return o != null && i != null ? this.store.find(o, i) : null;
  });

  /** The conversation's name, for its picture's initials. */
  protected barName(c: Conversation): string {
    return this.store.title(c);
  }

  // The bar's name for the conversation. A deep link can render before the list
  // arrives, so it starts as a stand-in.
  private readonly barTitle = computed(() => {
    if (!this.routed()) return undefined;
    const c = this.conversation();
    return c ? this.store.title(c) : { text: 'Conversation', provisional: true };
  });

  /** Search within this conversation; see thread-search.ts. */
  protected readonly search = new ThreadSearch(this.api, this.origin, this.id);

  /** All fetched messages, ascending by ts; only a window is rendered. */
  // dev-lint: allow-component-list — `messages` is an infinite-scroll pagination
  // buffer, not a retained catalog; a thread re-fetches fresh on entry by design.
  readonly messages = signal<Message[]>([]);

  /** Which messages are in the DOM; see thread-window.ts. */
  private readonly win = new ThreadWindow(
    this.host,
    () => this.messagesEl()?.nativeElement,
    this.messages,
    () => this.appRef.tick(),
  );

  readonly rendered = this.win.rendered;
  readonly renderCount = this.win.renderCount;
  readonly topSpacer = this.win.topSpacer;
  readonly bottomSpacer = this.win.bottomSpacer;

  readonly loadingThread = signal(false);
  readonly loadingOlder = signal(false);
  readonly loadingNewer = signal(false);
  readonly hasMore = signal(false);
  readonly threadError = signal(false);
  /** Where to continue backwards: the oldest loaded row. */
  private cursor: string | null = null;
  /** Where to continue forwards: the newest loaded row, while `floating`. */
  private newerCursor: string | null = null;

  private fromTimer: ReturnType<typeof setTimeout> | null = null;
  /** Whether the reader has scrolled, tapped or typed since the thread loaded.
   *  Until then a scroll is the app's own (a landing, the pages loaded around it),
   *  and the place the URL names is still where the reader is. */
  protected readerMoved = false;
  /** Pending re-check for a scroll whose work was deferred — see `deferScrollCheck`. */
  private recheck: ReturnType<typeof setTimeout> | null = null;

  /** Bumped whenever the thread is reset. A fetch answering an older generation
   *  belongs to a conversation the reader has left, and is dropped. */
  private generation = 0;

  /** The rendered window by day and run, and where runs start; see message-view.ts. */
  readonly dayGroups = computed(() => dayGroups(this.rendered()));
  readonly startsRun = computed(() => runStarts(this.rendered()));
  protected readonly isPlain = isPlain;

  /** Others are named, except in a DM, whose bar already says who. */
  readonly namesSenders = computed(() => this.conversation()?.kind !== 'dm');

  constructor() {
    scaffoldTitle(this.barTitle);

    // Reload when the routed conversation changes; Angular reuses this instance.
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

    // A changed `?at` or `?on` reloads too, which the effect above cannot see:
    // clicking a hit in the open conversation changes only the query string.
    // Only a change to a non-null value counts; `commitFromParam` clearing it is
    // not a navigation.
    let landedAt: string | null = this.route.snapshot.queryParamMap.get('at');
    let landedOn: string | null = this.route.snapshot.queryParamMap.get('on');
    this.route.queryParamMap.pipe(takeUntilDestroyed()).subscribe((pm) => {
      const at = pm.get('at');
      const on = pm.get('on');
      const movedTo = (at != null && at !== landedAt) || (on != null && on !== landedOn);
      landedAt = at;
      landedOn = on;
      if (!movedTo) return;
      this.reload();
    });

    const poll = setInterval(() => void this.pollNewer(), POLL_MS);
    const destroyRef = inject(DestroyRef);
    // Intervals and timers go with the component.
    destroyRef.onDestroy(() => clearInterval(poll));
    destroyRef.onDestroy(() => {
      if (this.recheck != null) clearTimeout(this.recheck);
      // The `?from` debounce navigates, so it must not fire after leaving.
      if (this.fromTimer != null) clearTimeout(this.fromTimer);
      this.fetches.reset();
    });

    // The soft keyboard resizes the scroll container; see
    // `ThreadWindow.observeShrink`.
    destroyRef.onDestroy(this.win.observeShrink());
    // Re-pointed at the current message block after each re-render.
    effect(() => this.win.watchContent(this.messagesEl()?.nativeElement));
  }

  /** The loaded window does not reach the newest message: `?at` or `?on` put the
   *  reader in the middle. `pollNewer` must not run then, or its gap guard would
   *  reload the thread into the present. */
  readonly floating = signal(false);

  /** The message the reader was put on by `?at` or `jumpToReply`, marked so it
   *  can be told from its neighbours. Cleared on the first scroll. */
  readonly landedId = signal<string | null>(null);

  private resetState(): void {
    this.generation++;
    this.readerMoved = false;
    this.messages.set([]);
    this.win.reset();
    this.revealedIds.set(new Set());
    this.openHistories.set(new Set());
    this.fetches.reset();
    this.loadingOlder.set(false);
    this.loadingNewer.set(false);
    this.hasMore.set(false);
    this.floating.set(false);
    this.landedId.set(null);
    this.cursor = null;
    this.newerCursor = null;
  }

  /** Land on a message: half a page either side, fetched together. The forward
   *  half is `at`, which includes the cursor's own row; `older` and `newer` are
   *  both strict. */
  private async loadAround(
    origin: Origin,
    id: string,
    /** A cursor from a search hit or reply, or a day, passed as `on` for the
     *  server to convert to the origin's unit. */
    where: { at: string } | { onDay: number },
  ): Promise<boolean> {
    const half = Math.floor(PAGE / 2);
    const at = 'at' in where ? where.at : undefined;
    const on = 'onDay' in where ? where.onDay : undefined;
    const gen = this.generation;
    const [older, newer] = await Promise.all([
      firstValueFrom(this.api.messages(origin, id, at, half, 'older', on)),
      firstValueFrom(this.api.messages(origin, id, at, half, 'at', on)),
    ]);
    // Another load has begun; it owns the thread now.
    if (gen !== this.generation) return true;

    // An unreadable cursor makes the halves the two ends of the archive, out of
    // order. Their order is the only evidence, so check it and let the caller
    // fall back to the newest page.
    const lastOld = older.messages.at(-1);
    const firstNew = newer.messages[0];
    if (lastOld && firstNew && lastOld.ts > firstNew.ts) return false;

    this.messages.set([...older.messages, ...newer.messages]);
    this.fetches.watchInFlight([...older.messages, ...newer.messages]);
    this.hasMore.set(older.has_more);
    this.cursor = older.next_cursor;
    this.newerCursor = newer.prev_cursor;
    // Floating unless the forward half reached the present.
    this.floating.set(newer.has_more);
    this.loadingThread.set(false);
    this.appRef.tick();
    // The forward half starts with the hit.
    const hit = newer.messages[0];
    this.landedId.set(hit ? hit.id : null);
    this.win.withScrollLock(() => {
      if (hit) this.win.scrollToTs(hit.ts);
      else this.win.scrollToBottom();
    });
    this.win.trimToWindow();
    return true;
  }

  // How a message is shown; see message-view.ts.
  protected readonly reactors = reactors;
  protected deliveryLabel(d: Delivery): string {
    return deliveryLabel(d, this.conversation()?.kind);
  }
  protected readers(d: Delivery): string {
    return readers(d, this.locale);
  }
  protected readonly deliveryPending = deliveryPending;
  protected readonly segments = segments;
  protected readonly hostOf = hostOf;
  protected readonly mib = mib;

  /** The last day the calendar offers. */
  protected readonly today = new Date();

  /** Jump to a day, which the calendar gives as local midnight; the server
   *  converts `?on` to the origin's unit. In place of where the reader was, so
   *  back leaves the conversation, as from anywhere in it. */
  protected jumpToDate(day: Date | null): void {
    if (!day) return;
    void this.router.navigate([], {
      relativeTo: this.route,
      queryParams: { on: String(day.getTime()), at: null, from: null },
      queryParamsHandling: 'merge',
      replaceUrl: true,
    });
  }

  /** Go to a hit via `?at`, as a global search result does: it may be outside
   *  the rendered window. */
  protected openHit(h: SearchHit): void {
    this.search.toggle();
    void this.router.navigate(['/conversation', h.origin, h.conversation_id], {
      queryParams: { at: h.cursor, from: null, on: null },
      queryParamsHandling: 'merge',
      // A move within the conversation, as every jump in it is: back leaves it.
      replaceUrl: true,
    });
  }

  /** Go to the message a reply answers: a scroll if it is rendered, else a `?at`
   *  landing, since `scrollToTs` would pick the nearest rendered message. */
  jumpToReply(r: ReplyTo): void {
    if (!r.id) return;
    const here = this.rendered().find((m) => m.id === r.id);
    if (here) {
      this.landedId.set(here.id);
      this.win.withScrollLock(() => this.win.scrollToTs(here.ts));
      return;
    }
    // `cursor` comes with `id`; without one, stay rather than open the newest page.
    if (!r.cursor) return;
    void this.router.navigate([], {
      relativeTo: this.route,
      // `from` goes too; it would name a different place.
      queryParams: { at: r.cursor, from: null, on: null },
      queryParamsHandling: 'merge',
      replaceUrl: true,
    });
  }

  /** Load the thread. Restores the paged-back depth from ?from (the ts the user
   *  was looking at) so a refresh/deep-link returns to the same spot; otherwise
   *  opens pinned to the latest message, like a chat app. */
  private async loadThread(origin: Origin, id: string): Promise<void> {
    this.resetState();
    const gen = this.generation;
    this.threadError.set(false);
    this.loadingThread.set(true);
    const at = this.route.snapshot.queryParamMap.get('at');
    const onDay = Number(this.route.snapshot.queryParamMap.get('on')) || null;
    const from = Number(this.route.snapshot.queryParamMap.get('from')) || null;
    try {
      // `?at` means "put me here" and `?from` "I was here"; `?at` wins, and
      // `commitFromParam` replaces it with `from` on the first scroll.
      if (at != null && (await this.loadAround(origin, id, { at }))) return;
      // `?on`, a picked date, likewise.
      if (onDay != null && (await this.loadAround(origin, id, { onDay }))) return;
      const first = await firstValueFrom(this.api.messages(origin, id, undefined, PAGE));
      if (gen !== this.generation) return;
      let msgs = first.messages;
      let hasMore = first.has_more;
      let cursor = first.next_cursor;
      // Page older to the saved depth, at most `MAX_RESTORE_PAGES`: `from` is a
      // timestamp, possibly thousands of pages back. `scrollToTs` falls back to
      // the oldest loaded message.
      let pages = 0;
      while (
        from != null &&
        hasMore &&
        cursor != null &&
        msgs.length > 0 &&
        msgs[0].ts > from &&
        pages < MAX_RESTORE_PAGES
      ) {
        pages++;
        const older = await firstValueFrom(this.api.messages(origin, id, cursor, PAGE));
        if (gen !== this.generation) return;
        if (older.messages.length === 0) break;
        msgs = [...older.messages, ...msgs];
        hasMore = older.has_more;
        cursor = older.next_cursor;
      }
      this.messages.set(msgs);
      this.fetches.watchInFlight(msgs);
      this.hasMore.set(hasMore);
      this.cursor = cursor;
      this.loadingThread.set(false);
      this.appRef.tick();
      // Position the viewport, then bound the DOM around it.
      this.win.withScrollLock(() => {
        if (from != null) this.win.scrollToTs(from);
        else this.win.scrollToBottom();
      });
      this.win.trimToWindow();
      // `watchContent` holds the bottom as images load.
    } catch {
      if (gen !== this.generation) return;
      this.threadError.set(true);
      this.loadingThread.set(false);
    }
  }

  reload(): void {
    void this.reloadNow();
  }

  private async reloadNow(): Promise<void> {
    const o = this.origin();
    const i = this.id();
    if (o != null && i != null) await this.loadThread(o, i);
  }

  // ---- copying a selection as a chat log -----------------------------------

  /** Copy a selection of two or more messages as an irssi-style log, in both
   *  `text/plain` and `text/html`. Every copy route (⌘C, the context menu,
   *  Android's selection bar) arrives as this event. Below two messages the
   *  browser's own copy stands. */
  onCopy(e: ClipboardEvent): void {
    const data = e.clipboardData;
    if (data == null) return;
    const picked = selectedMessages(this.messagesEl()?.nativeElement, document.getSelection(), this.rendered());
    if (picked.length < 2) return;
    // The conversation's size, only when the selection is the whole rendered
    // window (a truncated select-all). Absent before the list loads.
    const total = picked.length === this.rendered().length ? this.conversation()?.message_count : undefined;
    const text = formatChatLog(picked, total != null ? { total } : undefined);
    data.setData('text/plain', text);
    data.setData('text/html', chatLogHtml(text));
    e.preventDefault();
  }

  // ---- keeping up with what arrives ---------------------------------------

  /** Guards against a second poll starting while one is still in flight — a
   *  slow response must not be overtaken by the tick behind it. */
  private polling = false;

  /** Merge in anything newer, using the newest-page endpoint. Silent on failure:
   *  the next tick retries. */
  async pollNewer(): Promise<void> {
    const o = this.origin();
    const i = this.id();
    if (o == null || i == null) return;
    // Not during a load or a send, while floating (see `floating`), or while hidden.
    if (this.polling || this.loadingThread() || this.loadingOlder() || this.composer.sending()) return;
    if (this.floating()) return;
    if (document.visibilityState !== 'visible') return;
    if (this.messages().length === 0) return;

    this.polling = true;
    const gen = this.generation;
    try {
      const page = await firstValueFrom(this.api.messages(o, i, undefined, PAGE));
      // The reader may have switched conversations meanwhile.
      if (gen !== this.generation) return;

      const held = this.messages();
      const known = new Set(held.map((m) => m.id));
      const fresh = page.messages.filter((m) => !known.has(m.id));
      if (fresh.length === 0) return;

      // None of the newest page is known: more than a page arrived, so reload
      // rather than leave a hole.
      if (fresh.length === page.messages.length) {
        await this.loadThread(o, i);
        return;
      }

      const wasAtBottom = this.win.atBottom();
      // Sorted: an import can land an older line late.
      this.messages.set(
        [...held, ...fresh].sort((a, b) => a.ts - b.ts || a.id.localeCompare(b.id)),
      );
      this.appRef.tick();
      // Follow only a reader already at the end.
      if (wasAtBottom) this.win.withScrollLock(() => this.win.scrollToBottom());
      this.win.trimToWindow();
    } catch {
      // Not an error state.
    } finally {
      this.polling = false;
    }
  }

  // ---- sending (IRC only) -------------------------------------------------

  /** Only IRC has a client to send with. */
  readonly canSend = computed(() => this.origin() === 'irc' && this.routed());
  /** The draft and its sending; see composer.ts. A line irssi logged reloads the
   *  thread, to show it. */
  readonly composer = new Composer(this.api, this.origin, this.id, () => this.reloadNow());

  // ---- what the reader asks to be fetched ----------------------------------

  /** What the reader asked to be fetched; see fetch-requests.ts. */
  protected readonly fetches = new FetchRequests(this.api, this.messages);

  // ---- scrolling ----------------------------------------------------------

  /** The window engine decides what to reveal or collapse; fetching stays here. */
  onScroll(): void {
    // Before any early return; see `ThreadWindow.noteScroll`.
    this.win.noteScroll();
    if (this.threadError() || !this.routed()) return;
    // Deferred, not dropped: a programmatic scroll delivers one event, and the
    // fetch or `?from` it calls for must still happen once the guard clears.
    if (this.win.busy || this.loadingThread()) {
      this.deferScrollCheck();
      return;
    }
    const { needOlder, needNewer } = this.win.step();
    if (needOlder) this.fetchOlder();
    if (needNewer) this.fetchNewer();
    this.scheduleFromParam();
  }

  /** Re-run `onScroll` once the guard that skipped it clears; one timer at a time. */
  private deferScrollCheck(): void {
    if (this.recheck != null) return;
    this.recheck = setTimeout(() => {
      this.recheck = null;
      this.onScroll();
    }, 60);
  }

  fetchOlder(): void {
    const o = this.origin();
    const i = this.id();
    if (o == null || i == null || !this.hasMore() || this.cursor == null || this.loadingOlder()) return;
    this.loadingOlder.set(true);
    const gen = this.generation;
    this.api.messages(o, i, this.cursor, PAGE).subscribe({
      next: (page) => {
        if (gen !== this.generation) return;
        // Prepend, keeping the viewport on the same message.
        this.win.keepingAnchor('fetchOlder', () => {
          this.messages.update((cur) => [...page.messages, ...cur]);
          this.hasMore.set(page.has_more);
          this.cursor = page.next_cursor;
          this.loadingOlder.set(false);
        });
        this.win.enforceMax('bottom');
        this.scheduleFromParam();
      },
      error: () => {
        if (gen === this.generation) this.loadingOlder.set(false);
      },
    });
  }

  /** Grow the window forwards. When the forward page runs out the window has
   *  reached the present: `floating` goes false and polling resumes. */
  fetchNewer(): void {
    const o = this.origin();
    const i = this.id();
    // Only while floating; otherwise there is nothing newer to fetch.
    if (o == null || i == null || !this.floating()) return;
    if (this.newerCursor == null || this.loadingNewer()) return;
    this.loadingNewer.set(true);
    const gen = this.generation;
    this.api.messages(o, i, this.newerCursor, PAGE, 'newer').subscribe({
      next: (page) => {
        if (gen !== this.generation) return;
        // Append, keeping the viewport on the same message.
        this.win.keepingAnchor('fetchNewer', () => {
          this.messages.update((cur) => [...cur, ...page.messages]);
          this.newerCursor = page.prev_cursor;
          this.floating.set(page.has_more);
          this.loadingNewer.set(false);
        });
        this.win.enforceMax('top');
        this.scheduleFromParam();
      },
      error: () => {
        if (gen === this.generation) this.loadingNewer.set(false);
      },
    });
  }

  // ---- ?from (scroll-position restore) -----------------------------------

  private scheduleFromParam(): void {
    if (this.fromTimer) clearTimeout(this.fromTimer);
    this.fromTimer = setTimeout(() => this.commitFromParam(), 300);
  }

  commitFromParam(): void {
    // Where the viewport is, whoever moved it. The landing (its marker, `?at`, a
    // picked day) goes only once the reader has: the pages the app loads around
    // it by itself scroll too, and on a phone at once.
    const left = this.readerMoved;
    if (left) this.landedId.set(null);
    const o = this.origin();
    const i = this.id();
    if (o == null || i == null) return;
    const from = this.win.atBottom() ? null : this.win.topAnchor()?.id;
    const ts = from ? this.messages().find((m) => m.id === from)?.ts : null;
    const landing = left ? { at: null, on: null } : {};
    void this.router.navigate([], {
      relativeTo: this.route,
      queryParams: { from: ts != null ? String(ts) : null, ...landing },
      queryParamsHandling: 'merge',
      replaceUrl: true,
    });
  }
}

/** `set` with `id` added if absent, removed if present. */
function toggled(set: ReadonlySet<string>, id: string): ReadonlySet<string> {
  const next = new Set(set);
  if (!next.delete(id)) next.add(id);
  return next;
}
