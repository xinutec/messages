import { ApplicationRef, Component, DestroyRef, ElementRef, LOCALE_ID, computed, effect, inject, input, signal, viewChild } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { DatePipe, Location, NgTemplateOutlet } from '@angular/common';
import { ActivatedRoute, Router } from '@angular/router';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatListModule } from '@angular/material/list';
import { MatMenuModule } from '@angular/material/menu';
import { MatCalendar } from '@angular/material/datepicker';
import { TextFieldModule } from '@angular/cdk/text-field';
import { FormsModule } from '@angular/forms';
import { MatSnackBar } from '@angular/material/snack-bar';
import { Pictures, ScaffoldActions, ScaffoldContext, ScaffoldLeading, scaffoldTitle } from '@xinutec/ui-scaffold';

import { firstValueFrom } from 'rxjs';

import { attachmentName, attachmentNoun, isAudio, isImage, isVideo } from './attachment';
import { Avatar } from './avatar';
import { FetchRequests } from './fetch-requests';
import { segments } from './formatting';
import { hue } from './list-row';
import { clipboardOf, writeClipboard } from './copy-log';
import { Picking, selectedMessages } from './selection';
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
import { ReaderPlace } from './reader-place';
import { ThreadSearch } from './thread-search';
import { Composer } from './composer';
import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
import { Attachment, Conversation, Delivery, Message, MessagesPage, Origin, ReplyTo, SearchHit } from './models';

/** How often an open, visible thread checks for newer messages. */
const POLL_MS = 5000;

/** How long after the last scroll the floating day pill stays. */
const SCROLL_IDLE_MS = 1200;

/** How long without a scroll ends the reader's gesture: past a momentum
 *  scroll's frames, short of the thread's own next move. */
const READER_IDLE_MS = 200;

/** Keys that scroll the list when focus is not in a field. */
const SCROLL_KEYS = new Set(['PageUp', 'PageDown', 'Home', 'End', 'ArrowUp', 'ArrowDown', ' ']);

@Component({
  selector: 'app-thread',
  templateUrl: './thread.html',
  styleUrls: ['./thread.scss', './thread-media.scss', './thread-message.scss'],
  // A column: the scroll container (`.thread`, which the sticky headers pin
  // against), then the composer below it, so a growing draft shortens the list
  // instead of covering it. `copy` bubbles here from wherever the selection is.
  host: {
    '(copy)': 'onCopy($event)',
    // The page keys scroll the list from wherever focus is; see `readerInput`.
    '(document:keydown)': 'readerInput($event)',
    '(document:pointerup)': 'readerLetGo()',
    // Picking messages by hand; see `picking`. On the host rather than the
    // bubbles: the bubble is found from the event, and nothing else here is one.
    '(contextmenu)': 'picking.press($event)',
    '(click)': 'picking.tap($event)',
    '(document:keydown.escape)': 'picking.clear()',
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
    ScaffoldContext,
    ScaffoldLeading,
    Avatar,
    MatMenuModule,
    MatCalendar,
    TextFieldModule,
  ],
})
export class Thread {
  private api = inject(MessagesApi);
  private store = inject(MessagesStore);
  private router = inject(Router);
  private route = inject(ActivatedRoute);
  private location = inject(Location);
  private appRef = inject(ApplicationRef);
  private locale = inject(LOCALE_ID);
  private snack = inject(MatSnackBar);
  private readonly messagesEl = viewChild<ElementRef<HTMLElement>>('messagesEl');
  private readonly scrollerEl = viewChild<ElementRef<HTMLElement>>('scroller');

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
  readonly messages = signal<Message[]>([]);

  /** Which messages are in the DOM; see thread-window.ts. */
  private readonly win = new ThreadWindow(
    () => this.scrollerEl()?.nativeElement,
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
  /** Where to continue forwards: the newest loaded row. Read while `floating`,
   *  and kept up always, so a poll that finds a gap can float from it. */
  private newerCursor: string | null = null;

  private fromTimer: ReturnType<typeof setTimeout> | null = null;

  // ---- whose scroll it is ----------------------------------------------------
  //
  // A scroll is the reader's only while a scrolling gesture of theirs is under
  // way (`readerInput`): the wheel, a finger dragging, the scrollbar held, a page
  // key. It lasts until the scrolling stops, so a momentum scroll counts. The
  // thread's own scrolls, a resize, a picture loading and a tap or a key in the
  // box are not the reader's, and decide nothing about where they are.

  /** A scrolling gesture of the reader's is under way. */
  private readerScrolling = false;
  /** The scrollbar is held; the gesture lasts until it is let go. */
  private holdingScrollbar = false;
  private readerIdle: ReturnType<typeof setTimeout> | null = null;

  /** Input that starts the reader's scrolling, and nothing else: a tap on a
   *  message or a key in the box touches the thread without moving it. */
  protected readerInput(e: Event): void {
    const list = this.scrollerEl()?.nativeElement;
    const scrolls =
      e.type === 'wheel' ||
      e.type === 'touchmove' ||
      // On the list itself, not a bubble in it: its scrollbar.
      (e.type === 'pointerdown' && e.target === list) ||
      (e instanceof KeyboardEvent && SCROLL_KEYS.has(e.key) && !typingIn(e.target));
    if (!scrolls) return;
    if (e.type === 'pointerdown') this.holdingScrollbar = true;
    this.readerScrolling = true;
    this.armReaderIdle();
  }

  protected readerLetGo(): void {
    if (!this.holdingScrollbar) return;
    this.holdingScrollbar = false;
    this.armReaderIdle();
  }

  private armReaderIdle(): void {
    if (this.readerIdle != null) clearTimeout(this.readerIdle);
    this.readerIdle = setTimeout(() => {
      this.readerIdle = null;
      if (!this.holdingScrollbar) this.readerScrolling = false;
    }, READER_IDLE_MS);
  }

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
      this.picking.clear();
      this.composer.switchTo(key);
      if (o != null && i != null) void this.loadThread(o, i);
      else this.resetState();
    });

    // A changed `?at` or `?on` reloads too, which the effect above cannot see:
    // clicking a hit in the open conversation changes only the query string.
    this.route.queryParamMap.pipe(takeUntilDestroyed()).subscribe((pm) => {
      if (this.place.isNewLanding(pm)) this.reload();
    });

    const poll = setInterval(() => void this.pollNewer(), POLL_MS);
    const destroyRef = inject(DestroyRef);
    // Intervals and timers go with the component.
    destroyRef.onDestroy(() => clearInterval(poll));
    // Back while picking pops the entry the picking added, and ends it.
    const popped = this.location.subscribe(() => this.picking.clear());
    destroyRef.onDestroy(() => popped.unsubscribe());
    destroyRef.onDestroy(() => {
      if (this.readerIdle != null) clearTimeout(this.readerIdle);
      if (this.scrollIdle != null) clearTimeout(this.scrollIdle);
      // The `?from` debounce navigates, so it must not fire after leaving.
      if (this.fromTimer != null) clearTimeout(this.fromTimer);
      this.fetches.reset();
    });

    // The soft keyboard and the composer resize the scroll container; see
    // `ThreadWindow.observeShrink`.
    destroyRef.onDestroy(this.win.observeShrink());
    effect(() => this.win.watchScroller(this.scrollerEl()?.nativeElement));
    // Re-pointed at the current message block after each re-render.
    effect(() => this.win.watchContent(this.messagesEl()?.nativeElement));
  }

  /** Where the reader is, and why; see reader-place.ts. */
  private readonly place = new ReaderPlace(this.route.snapshot.queryParamMap);
  /** See `ReaderPlace.floating`. */
  readonly floating = this.place.floating;
  /** See `ReaderPlace.landedId`. */
  readonly landedId = this.place.landedId;

  private resetState(): void {
    this.generation++;
    this.place.reset();
    this.messages.set([]);
    this.win.reset();
    this.revealedIds.set(new Set());
    this.openHistories.set(new Set());
    this.fetches.reset();
    this.loadingOlder.set(false);
    this.loadingNewer.set(false);
    this.hasMore.set(false);
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
    // The forward half starts with the hit; floating unless it reached the present.
    const hit = newer.messages[0];
    this.place.landOn(hit ? hit.id : null, newer.has_more);
    this.loadingThread.set(false);
    this.appRef.tick();
    if (hit) this.win.scrollToTs(hit.ts);
    else this.win.scrollToBottom();
    this.win.trimToWindow();
    this.stepWindow();
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
      // A landing of its own, in the window already loaded.
      this.place.landOn(here.id, this.floating());
      this.win.scrollToTs(here.ts);
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
    // The kept page on view, if any, as a key; see `showKept`.
    let shown: string | null = null;
    try {
      // `?at` means "put me here" and `?from` "I was here"; `?at` wins, and
      // `commitFromParam` replaces it with `from` once the reader scrolls.
      if (at != null && (await this.loadAround(origin, id, { at }))) return;
      // `?on`, a picked date, likewise.
      if (onDay != null && (await this.loadAround(origin, id, { onDay }))) return;
      if (from == null) {
        shown = await this.showKept(origin, id, gen);
        if (gen !== this.generation) return;
      }
      const page = await this.fetchToDepth(origin, id, from, gen);
      if (page == null) return;
      this.hasMore.set(page.has_more);
      this.cursor = page.next_cursor;
      this.newerCursor = page.prev_cursor;
      if (shown != null && shown === JSON.stringify(page.messages)) {
        // What is on view is the network's page already.
        this.loadingThread.set(false);
        return;
      }
      const moved = shown != null && this.place.hasScrolled;
      this.messages.set(page.messages);
      this.fetches.watchInFlight(page.messages);
      this.loadingThread.set(false);
      this.appRef.tick();
      // Position the viewport, then bound the DOM around it.
      if (from != null) this.win.scrollToTs(from);
      else if (!moved) this.win.scrollToBottom();
      this.win.trimToWindow();
      this.stepWindow();
      // `watchContent` holds the bottom as images load.
    } catch (e: unknown) {
      if (gen !== this.generation) return;
      this.loadingThread.set(false);
      // The kept page stays on view; the shell says it is what was last seen.
      if (shown != null) {
        this.store.noteFailure(e);
        return;
      }
      this.threadError.set(true);
    }
  }

  /** A plain open shows the newest page as the service worker last kept it,
   *  under the bar, until the network's page comes; the same page then changes
   *  nothing, and a different one replaces it, moving the view only if the
   *  reader has not. Returns what it put on view, as a key, or null. */
  private async showKept(origin: Origin, id: string, gen: number): Promise<string | null> {
    const kept = await this.api.keptMessages(origin, id, PAGE);
    if (gen !== this.generation || kept == null || kept.messages.length === 0) return null;
    this.messages.set(kept.messages);
    this.newerCursor = kept.prev_cursor;
    this.fetches.watchInFlight(kept.messages);
    this.appRef.tick();
    this.win.scrollToBottom();
    return JSON.stringify(kept.messages);
  }

  /** The newest page, paged older to the saved depth `from`, at most
   *  `MAX_RESTORE_PAGES`: `from` is a timestamp, possibly thousands of pages
   *  back, and `scrollToTs` falls back to the oldest loaded message. Null when
   *  another load took over meanwhile. */
  private async fetchToDepth(
    origin: Origin,
    id: string,
    from: number | null,
    gen: number,
  ): Promise<MessagesPage | null> {
    const first = await firstValueFrom(this.api.messages(origin, id, undefined, PAGE));
    if (gen !== this.generation) return null;
    let { messages, has_more, next_cursor } = first;
    let pages = 0;
    while (
      from != null &&
      has_more &&
      next_cursor != null &&
      messages.length > 0 &&
      messages[0].ts > from &&
      pages < MAX_RESTORE_PAGES
    ) {
      pages++;
      const older = await firstValueFrom(this.api.messages(origin, id, next_cursor, PAGE));
      if (gen !== this.generation) return null;
      if (older.messages.length === 0) break;
      messages = [...older.messages, ...messages];
      has_more = older.has_more;
      next_cursor = older.next_cursor;
    }
    return { ...first, messages, has_more, next_cursor };
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
    const { text, html } = clipboardOf(picked, this.scopeOf(picked));
    data.setData('text/plain', text);
    if (html != null) data.setData('text/html', html);
    e.preventDefault();
  }

  /** The conversation's size, only when the copy is the whole rendered window
   *  (a truncated select-all). Absent before the list loads. */
  private scopeOf(picked: readonly Message[]): { total: number } | undefined {
    const total = picked.length === this.rendered().length ? this.conversation()?.message_count : undefined;
    return total != null ? { total } : undefined;
  }

  // ---- picking messages by hand -------------------------------------------

  /** Messages picked by hand, for the bar and the copy; see selection.ts. When
   *  it ends, the URL takes the scroll depth it did not while picking
   *  (`commitFromParam`). */
  readonly picking = new Picking(this.messages, this.location, () => this.scheduleFromParam());

  async copyPicked(): Promise<void> {
    const picked = this.picking.picked();
    const { text, html } = clipboardOf(picked, this.scopeOf(picked));
    try {
      await writeClipboard(text, html);
    } catch {
      this.snack.open("Couldn't copy", undefined, { duration: 3000 });
      return;
    }
    this.snack.open(picked.length === 1 ? 'Copied' : `Copied ${picked.length} messages`, undefined, { duration: 2000 });
    this.picking.clear();
  }

  // ---- keeping up with what arrives ---------------------------------------

  /** Guards against a second poll starting while one is still in flight — a
   *  slow response must not be overtaken by the tick behind it. */
  private polling = false;

  /** Merge in anything newer, and refresh what is held, from the newest page.
   *  A reader landed in the past (`floating`) is not polled, so a message there
   *  shows its state as of the landing. Silent on failure: the next tick
   *  retries. */
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
      this.store.noteAnswer();
      // The reader may have switched conversations meanwhile.
      if (gen !== this.generation) return;

      const held = this.messages();
      const known = new Set(held.map((m) => m.id));
      const fresh = page.messages.filter((m) => !known.has(m.id));

      // None of the newest page is known: more than a page arrived. A reader at
      // the end wants the present, so reload; one back in history keeps their
      // place, and the rest comes forwards as they scroll (`fetchNewer`).
      if (fresh.length > 0 && fresh.length === page.messages.length) {
        if (this.win.isFollowing()) await this.loadThread(o, i);
        else this.place.reachesPresent(false);
        return;
      }

      // A held message as it is now: deleted, edited or reacted to since it
      // loaded. The newest page is where that happens; an unchanged one stays
      // the same object, so its bubble is not redrawn.
      const now = new Map(page.messages.map((m) => [m.id, m]));
      let changed = false;
      const current = held.map((m) => {
        const fresher = now.get(m.id);
        if (fresher == null || JSON.stringify(fresher) === JSON.stringify(m)) return m;
        changed = true;
        return fresher;
      });
      if (fresh.length === 0 && !changed) return;

      const following = this.win.isFollowing();
      if (fresh.length > 0) this.newerCursor = page.prev_cursor;
      // Sorted: an import can land an older line late.
      this.messages.set(
        [...current, ...fresh].sort((a, b) => a.ts - b.ts || a.id.localeCompare(b.id)),
      );
      this.appRef.tick();
      // Follow only a reader already at the end.
      if (following) this.win.scrollToBottom();
      this.win.trimToWindow();
    } catch (e: unknown) {
      // Not an error state; a poll with no network is what makes the view stale.
      this.store.noteFailure(e);
    } finally {
      this.polling = false;
    }
  }

  // ---- sending (IRC and Telegram) -----------------------------------------

  /** IRC, and a Telegram DM or group: not a broadcast, which has an audience
   *  rather than a conversation. Signal is read-only by decision. */
  readonly canSend = computed(() => {
    if (!this.routed()) return false;
    if (this.origin() === 'irc') return true;
    const kind = this.conversation()?.kind;
    return this.origin() === 'telegram' && (kind === 'dm' || kind === 'group');
  });
  /** The draft and its sending; see composer.ts. */
  readonly composer = new Composer(this.api, this.origin, this.id, () => this.afterSend());

  /** Show what was just sent: the newest page merged in and the view at the
   *  end, with no reload; a window floating in the past is reloaded into the
   *  present instead, as `pollNewer` would not run there. */
  private async afterSend(): Promise<void> {
    if (this.floating()) return this.reloadNow();
    await this.pollNewer();
    this.win.scrollToBottom();
  }

  // ---- what the reader asks to be fetched ----------------------------------

  /** What the reader asked to be fetched; see fetch-requests.ts. */
  protected readonly fetches = new FetchRequests(this.api, this.messages);

  // ---- scrolling ----------------------------------------------------------

  /** The window engine decides what to reveal or collapse; fetching stays here. */
  onScroll(): void {
    // Whoever moved it, the window follows the view; only the reader's own
    // scroll says where the reader is (see "whose scroll it is"). One the window
    // made, a page loaded during the gesture, neither counts nor prolongs it.
    if (this.readerScrolling && !this.win.placedThisScroll()) {
      this.armReaderIdle();
      this.place.readerScrolled();
      this.win.noteReaderAt(!this.floating());
      this.noteFloatingDay();
    }
    if (this.threadError() || !this.routed() || this.loadingThread()) return;
    this.stepWindow();
  }

  /** Reveal or fetch at whichever edge the view is near, and note the place.
   *  On every scroll, and once a load has placed the view: a scroll during the
   *  load is skipped, and a page shorter than the screen scrolls nothing. */
  private stepWindow(): void {
    const { needOlder, needNewer } = this.win.step();
    if (needOlder) void this.fetchOlder();
    if (needNewer) void this.fetchNewer();
    this.scheduleFromParam();
  }

  // ---- the day on view, while scrolling --------------------------------------

  /** The day of the message at the top of the view, and whether the reader is
   *  scrolling now: the floating day pill (thread.html) shows the one while the
   *  other holds, and fades `SCROLL_IDLE_MS` after the last scroll, as Signal's
   *  does. */
  readonly floatingDay = signal<number | null>(null);
  readonly scrolling = signal(false);
  private scrollIdle: ReturnType<typeof setTimeout> | null = null;

  private noteFloatingDay(): void {
    const top = this.win.topAnchor();
    const m = top == null ? undefined : this.messages().find((x) => x.id === top.id);
    this.floatingDay.set(m?.ts ?? null);
    this.scrolling.set(true);
    if (this.scrollIdle != null) clearTimeout(this.scrollIdle);
    this.scrollIdle = setTimeout(() => this.scrolling.set(false), SCROLL_IDLE_MS);
  }

  async fetchOlder(): Promise<void> {
    const o = this.origin();
    const i = this.id();
    if (o == null || i == null || !this.hasMore() || this.cursor == null || this.loadingOlder()) return;
    this.loadingOlder.set(true);
    const gen = this.generation;
    try {
      const page = await firstValueFrom(this.api.messages(o, i, this.cursor, PAGE));
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
    } catch (e: unknown) {
      if (gen !== this.generation) return;
      this.loadingOlder.set(false);
      // The next scroll asks again; no network is said by the shell.
      this.store.noteFailure(e);
    }
  }

  /** Grow the window forwards. When the forward page runs out the window has
   *  reached the present: `floating` goes false and polling resumes. */
  async fetchNewer(): Promise<void> {
    const o = this.origin();
    const i = this.id();
    // Only while floating; otherwise there is nothing newer to fetch.
    if (o == null || i == null || !this.floating()) return;
    if (this.newerCursor == null || this.loadingNewer()) return;
    this.loadingNewer.set(true);
    const gen = this.generation;
    try {
      const page = await firstValueFrom(this.api.messages(o, i, this.newerCursor, PAGE, 'newer'));
      if (gen !== this.generation) return;
      // Append, keeping the viewport on the same message.
      this.win.keepingAnchor('fetchNewer', () => {
        this.messages.update((cur) => [...cur, ...page.messages]);
        this.newerCursor = page.prev_cursor;
        this.place.reachesPresent(!page.has_more);
        this.loadingNewer.set(false);
      });
      this.win.enforceMax('top');
      this.scheduleFromParam();
    } catch (e: unknown) {
      if (gen !== this.generation) return;
      this.loadingNewer.set(false);
      this.store.noteFailure(e);
    }
  }

  // ---- ?from (scroll-position restore) -----------------------------------

  private scheduleFromParam(): void {
    if (this.fromTimer) clearTimeout(this.fromTimer);
    this.fromTimer = setTimeout(() => this.commitFromParam(), 300);
  }

  commitFromParam(): void {
    // Not while picking: this replaces the history entry on top, which is then
    // the picking's step, and the close would have none to take away. The
    // picking's end catches up.
    if (this.picking.picked().length > 0) return;
    const o = this.origin();
    const i = this.id();
    if (o == null || i == null) return;
    // Where the viewport is, whoever moved it; the landing stays in the address
    // until the reader scrolls (`ReaderPlace.address`).
    const top = this.win.atBottom() ? null : this.win.topAnchor()?.id;
    const ts = top ? (this.messages().find((m) => m.id === top)?.ts ?? null) : null;
    const change = this.place.address(this.route.snapshot.queryParamMap, ts);
    if (change == null) return;
    void this.router.navigate([], {
      relativeTo: this.route,
      queryParams: change,
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

/** Focus is in something that takes keys itself: the box, the search field. */
function typingIn(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName))
  );
}
