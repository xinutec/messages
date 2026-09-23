import { ApplicationRef, Component, DestroyRef, ElementRef, LOCALE_ID, computed, effect, inject, input, signal, viewChild } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { DatePipe, formatDate } from '@angular/common';
import { ActivatedRoute, Router } from '@angular/router';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatListModule } from '@angular/material/list';
import { FormsModule } from '@angular/forms';

import { Subject, catchError, firstValueFrom, of, switchMap } from 'rxjs';

import { attachmentName, attachmentNoun } from './attachment';
import { LogScope, chatLogHtml, formatChatLog } from './copy-log';
import { MAX_RESTORE_PAGES, PAGE, ThreadWindow } from './thread-window';
import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
import { Conversation, Delivery, LinkOffer, Message, Origin, Attachment, Reaction, ReplyTo, SearchHit } from './models';

/** How often an open, visible thread checks for newer messages. */
const POLL_MS = 5000;

/** How often a fetch in flight is asked about. */
const MEDIA_WATCH_INTERVAL_MS = 4000;

/** How long to watch a requested fetch before giving up quietly; reopening the
 *  conversation asks again. */
const MEDIA_WATCH_LIMIT_MS = 15 * 60 * 1000;


@Component({
  selector: 'app-thread',
  templateUrl: './thread.html',
  styleUrl: './thread.scss',
  // The host is the scroll container the sticky headers pin against. `copy`
  // bubbles here from wherever the selection is.
  host: { class: 'thread', '(scroll)': 'onScroll()', '(copy)': 'onCopy($event)' },
  imports: [
    DatePipe,
    FormsModule,
    MatButtonModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatListModule,
    MatProgressBarModule,
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

  /** Where an attachment's bytes come from: each origin has its own route, since
   *  attachment ids are per origin. */
  protected attachmentUrl(a: Attachment): string {
    // A Record, so a new origin is a type error rather than another origin's
    // endpoint.
    const route: Record<Origin, string> = {
      signal: 'attachments',
      gchat: 'gchat-attachments',
      telegram: 'telegram-media',
      // IRC has no attachments.
      irc: 'attachments',
    };
    const origin = this.origin();
    // Unreachable while routed; an empty src fails visibly.
    return origin ? `/api/${route[origin]}/${a.id}` : '';
  }
  protected readonly attachmentNoun = attachmentNoun;

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
    this.openHistories.update((cur) => {
      const next = new Set(cur);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  }

  protected toggleReveal(id: string): void {
    this.revealedIds.update((cur) => {
      const next = new Set(cur);
      if (!next.delete(id)) next.add(id);
      return next;
    });
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

  // A deep link can render before the list arrives.
  readonly headTitle = computed(() => {
    const c = this.conversation();
    return c ? this.store.title(c) : 'Conversation';
  });

  /** Search within this conversation. On a phone the shell's search box is
   *  hidden while a thread is open. */
  readonly searchOpen = signal(false);
  readonly searchQuery = signal('');
  readonly searchHits = signal<SearchHit[] | null>(null);
  readonly searchBusy = signal(false);
  /** A failed search, distinct from no hits. */
  readonly searchFailed = signal(false);
  private threadSearch$ = new Subject<string>();

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
  /** Where to continue BACKWARDS — the oldest loaded row. */
  private cursor: string | null = null;
  /** Where to continue forwards: the newest loaded row, while `floating`. */
  private newerCursor: string | null = null;

  private fromTimer: ReturnType<typeof setTimeout> | null = null;
  /** Pending re-check for a scroll whose work was deferred — see `deferScrollCheck`. */
  private recheck: ReturnType<typeof setTimeout> | null = null;



  /** Rendered messages grouped by day, each with a sticky date header. Within a
   *  day, adjacent members of one album (same `album`, same sender) form a run,
   *  drawn as one set; every other message is a run of one. */
  readonly dayGroups = computed(() => {
    const groups: { key: string; ts: number; runs: Message[][] }[] = [];
    let lastKey: string | null = null;
    for (const m of this.rendered()) {
      const key = new Date(m.ts).toDateString();
      if (key !== lastKey) {
        groups.push({ key, ts: m.ts, runs: [] });
        lastKey = key;
      }
      const runs = groups[groups.length - 1].runs;
      const prev = runs.at(-1)?.at(-1);
      if (m.album != null && prev?.album === m.album && prev.sender === m.sender) {
        runs[runs.length - 1].push(m);
      } else {
        runs.push([m]);
      }
    }
    return groups;
  });

  constructor() {
    // `switchMap`, so a slow answer cannot land after a newer one.
    this.threadSearch$
      .pipe(
        switchMap((q) => {
          const o = this.origin();
          const id = this.id();
          // Offered only on a routed conversation; never widened to global.
          if (o == null || id == null) return of<SearchHit[]>([]);
          return this.api.search(q, { origin: o, id }).pipe(
            catchError(() => {
              this.searchFailed.set(true);
              return of<SearchHit[]>([]);
            }),
          );
        }),
        takeUntilDestroyed(),
      )
      .subscribe((hits) => {
        this.searchHits.set(hits);
        this.searchBusy.set(false);
      });

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
      const o = this.origin();
      const i = this.id();
      if (o != null && i != null) void this.loadThread(o, i);
    });

    const poll = setInterval(() => void this.pollNewer(), POLL_MS);
    const destroyRef = inject(DestroyRef);
    // Intervals and timers go with the component.
    destroyRef.onDestroy(() => clearInterval(poll));
    destroyRef.onDestroy(() => {
      if (this.recheck != null) clearTimeout(this.recheck);
      // The `?from` debounce navigates, so it must not fire after leaving.
      if (this.fromTimer != null) clearTimeout(this.fromTimer);
      for (const tick of this.watching.values()) clearInterval(tick);
      this.watching.clear();
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
    this.messages.set([]);
    this.win.reset();
    this.revealedIds.set(new Set());
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
    const [older, newer] = await Promise.all([
      firstValueFrom(this.api.messages(origin, id, at, half, 'older', on)),
      firstValueFrom(this.api.messages(origin, id, at, half, 'at', on)),
    ]);

    // An unreadable cursor makes the halves the two ends of the archive, out of
    // order. Their order is the only evidence, so check it and let the caller
    // fall back to the newest page.
    const lastOld = older.messages.at(-1);
    const firstNew = newer.messages[0];
    if (lastOld && firstNew && lastOld.ts > firstNew.ts) return false;

    this.messages.set([...older.messages, ...newer.messages]);
    this.watchInFlight([...older.messages, ...newer.messages]);
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

  /** Hover text naming who reacted. `who` can be shorter than `count`; the rest
   *  are counted, never invented. */
  reactors(r: Reaction): string {
    if (!r.who.length) return '';
    const named = r.who.join(', ');
    const unnamed = r.count - r.who.length;
    return unnamed > 0 ? `${named} and ${unnamed} more` : named;
  }

  /** The tag on an outgoing message. Plain "read" only where it covers everyone
   *  (a DM, or Telegram's position); where people are named, "read by 2". An
   *  unloaded conversation kind gets the counted form. */
  protected deliveryLabel(d: Delivery): string {
    if (d.state !== 'read' && d.state !== 'viewed') return d.state;
    if (!d.read_by.length || this.conversation()?.kind === 'dm') return d.state;
    return `${d.state} by ${d.read_by.length}`;
  }

  /** Who read it and when; empty for Telegram. `formatDate` with LOCALE_ID, to
   *  match the `| date` beside it. */
  protected readers(d: Delivery): string {
    return d.read_by
      .map((r) => `${r.who} ${formatDate(r.at, 'short', this.locale)}`)
      .join(', ');
  }

  /** Dimmed until somebody has actually read it. */
  protected deliveryPending(d: Delivery): boolean {
    return d.state === 'sent' || d.state === 'delivered';
  }

  /** Cut a body into plain and formatted runs. Offsets are UTF-16 code units,
   *  which is what JavaScript indexes by. Overlapping runs are skipped, long ones
   *  clamped, and the text always comes from `body`, never the entity. */
  protected segments(m: Message): { text: string; kind: string; url: string | null }[] {
    const body = m.body ?? '';
    // `?.`: a message without the field must not blank the whole thread.
    if (!m.entities?.length) return [{ text: body, kind: 'plain', url: null }];
    const out: { text: string; kind: string; url: string | null }[] = [];
    let at = 0;
    for (const e of m.entities) {
      const start = Number(e.offset);
      const end = start + Number(e.length);
      if (!Number.isFinite(start) || start < at || start >= body.length) continue;
      const stop = Math.min(end, body.length);
      if (stop <= start) continue;
      if (start > at) out.push({ text: body.slice(at, start), kind: 'plain', url: null });
      out.push({ text: body.slice(start, stop), kind: e.kind, url: e.url });
      at = stop;
    }
    if (at < body.length) out.push({ text: body.slice(at), kind: 'plain', url: null });
    return out;
  }

  /** Where a formatted run points, or null. Reaches the DOM only through the
   *  sanitising `[href]`. */
  protected hrefOf(seg: { text: string; kind: string; url: string | null }): string | null {
    if (seg.kind === 'textUrl') return seg.url;
    if (seg.kind === 'url') return seg.text;
    if (seg.kind === 'email') return `mailto:${seg.text}`;
    return null;
  }

  /** Jump to a date: local midnight, not `Date.parse`, which reads a bare date as
   *  UTC. The server converts `?on` to the origin's unit. */
  protected jumpToDate(value: string): void {
    const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
    if (!m) return;
    const ms = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])).getTime();
    void this.router.navigate([], {
      relativeTo: this.route,
      queryParams: { on: String(ms), at: null, from: null },
      queryParamsHandling: 'merge',
    });
  }

  /** Open or close the in-thread search, clearing what it found. */
  protected toggleSearch(): void {
    const open = !this.searchOpen();
    this.searchOpen.set(open);
    if (!open) {
      this.searchQuery.set('');
      this.searchHits.set(null);
      this.searchFailed.set(false);
    }
  }

  protected runThreadSearch(): void {
    const q = this.searchQuery().trim();
    if (!q) {
      this.searchHits.set(null);
      return;
    }
    this.searchBusy.set(true);
    this.searchFailed.set(false);
    this.threadSearch$.next(q);
  }

  /** Go to a hit via `?at`, as a global search result does: it may be outside
   *  the rendered window. */
  protected openHit(h: SearchHit): void {
    this.toggleSearch();
    void this.router.navigate(['/conversation', h.origin, h.conversation_id], {
      queryParams: { at: h.cursor, from: null },
      queryParamsHandling: 'merge',
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
      queryParams: { at: r.cursor, from: null },
      queryParamsHandling: 'merge',
    });
  }

  /** Load the thread. Restores the paged-back depth from ?from (the ts the user
   *  was looking at) so a refresh/deep-link returns to the same spot; otherwise
   *  opens pinned to the latest message, like a chat app. */
  private async loadThread(origin: Origin, id: string): Promise<void> {
    this.resetState();
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
        if (older.messages.length === 0) break;
        msgs = [...older.messages, ...msgs];
        hasMore = older.has_more;
        cursor = older.next_cursor;
      }
      this.messages.set(msgs);
      this.watchInFlight(msgs);
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
      this.threadError.set(true);
      this.loadingThread.set(false);
    }
  }

  reload(): void {
    const o = this.origin();
    const i = this.id();
    if (o != null && i != null) void this.loadThread(o, i);
  }

  // ---- copying a selection as a chat log -----------------------------------

  /** Copy a selection of two or more messages as an irssi-style log, in both
   *  `text/plain` and `text/html`. Every copy route (⌘C, the context menu,
   *  Android's selection bar) arrives as this event. Below two messages the
   *  browser's own copy stands. */
  onCopy(e: ClipboardEvent): void {
    const data = e.clipboardData;
    if (data == null) return;
    const picked = this.selectedMessages();
    if (picked.length < 2) return;
    const text = formatChatLog(picked, this.copyScope(picked));
    data.setData('text/plain', text);
    data.setData('text/html', chatLogHtml(text));
    e.preventDefault();
  }

  /** The conversation's size, only when the selection is the whole rendered
   *  window (a truncated select-all). Absent before the list loads. */
  private copyScope(picked: Message[]): LogScope | undefined {
    if (picked.length !== this.rendered().length) return undefined;
    const total = this.conversation()?.message_count;
    return total != null ? { total } : undefined;
  }

  /** The rendered messages the selection touches, in order: `data-id` says which,
   *  the model says what. Only the rendered window can be selected. */
  private selectedMessages(): Message[] {
    const el = this.messagesEl()?.nativeElement;
    const sel = document.getSelection();
    if (el == null || sel == null || sel.isCollapsed) return [];
    // Firefox allows several ranges.
    const ranges = Array.from({ length: sel.rangeCount }, (_, i) => sel.getRangeAt(i));
    const ids = new Set<string>();
    for (const node of el.querySelectorAll<HTMLElement>('.msg[data-id]')) {
      // `intersectsNode`, not `containsNode`: a selection inside one bubble
      // selects that message. jsdom gets both wrong; e2e/copy.spec.ts covers it.
      if (ranges.some((r) => r.intersectsNode(node))) {
        const id = node.dataset['id'];
        if (id != null) ids.add(id);
      }
    }
    return this.rendered().filter((m) => ids.has(m.id));
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
    // Not during a load, nor while hidden.
    if (this.polling || this.loadingThread() || this.loadingOlder() || this.sending()) return;
    // Not while floating; see `floating`.
    if (this.floating()) return;
    if (document.visibilityState !== 'visible') return;
    if (this.messages().length === 0) return;

    this.polling = true;
    try {
      const page = await firstValueFrom(this.api.messages(o, i, undefined, PAGE));
      // The reader may have switched conversations meanwhile.
      if (this.origin() !== o || this.id() !== i) return;

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
  readonly draft = signal('');
  /** An IME candidate is in flight. `send` refuses then, since Enter in a form
   *  submits even when the keydown handler returned early. */
  readonly composing = signal(false);
  readonly sending = signal(false);
  /** irssi's refusal, shown as-is. */
  readonly sendError = signal<string | null>(null);

  async send(): Promise<void> {
    const o = this.origin();
    const i = this.id();
    const text = this.draft().trim();
    if (o == null || i == null || !text || this.sending() || this.composing()) return;

    this.sending.set(true);
    this.sendError.set(null);
    try {
      const res = await firstValueFrom(this.api.send(o, i, text));
      if (!res.sent) {
        this.sendError.set(res.error ?? 'Not sent.');
        return;
      }
      // Cleared only once sent, so a failure keeps what was typed.
      this.draft.set('');
      if (res.archived) {
        // Reload to show the line irssi logged.
        await this.loadThread(o, i);
      } else {
        this.sendError.set('Sent. It will appear here after the next import.');
      }
    } catch {
      this.sendError.set('Could not reach the server.');
    } finally {
      this.sending.set(false);
    }
  }

  /** Enter sends. The box is single-line: IRC cannot carry a newline. */
  onComposerKey(e: KeyboardEvent): void {
    // An IME is composing: Enter accepts its candidate. `keyCode 229` is how
    // older Android WebViews report it.
    if (e.isComposing || e.keyCode === 229) return;
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      void this.send();
    }
  }

  /** In-app back (the mobile single-pane control) = return to the list route,
   *  keeping the origin filter and dropping the paged depth. */
  back(): void {
    void this.router.navigate(['/'], { queryParams: { from: null }, queryParamsHandling: 'merge' });
  }

  // ---- pictures behind links -----------------------------------------------

  /** Link requests in flight; each answers on its own request. */
  private readonly asking = signal<ReadonlySet<string>>(new Set());

  /** Attachment id → interval, for requested fetches being watched. */
  private readonly watching = new Map<string, number>();

  protected isAsking(id: string): boolean {
    return this.asking().has(id);
  }

  /** Ask the feed for an unfetched attachment. The fetch is queued, so the answer
   *  is watched for rather than awaited. */
  protected requestMedia(a: Attachment): void {
    this.setAsking(a.id, true);
    this.api.requestTelegramMedia(a.id).subscribe({
      // 204 either way.
      next: () => this.watchMedia(a.id),
      error: () => this.setAsking(a.id, false),
    });
  }

  /** Poll an asked-for attachment until it arrives or fails, giving up quietly
   *  after `MEDIA_WATCH_LIMIT_MS`. */
  private watchMedia(id: string): void {
    if (this.watching.has(id)) return;
    const started = Date.now();
    const tick = window.setInterval(() => {
      if (Date.now() - started > MEDIA_WATCH_LIMIT_MS) {
        this.stopWatching(id);
        this.setAsking(id, false);
        return;
      }
      this.api.telegramMediaState(id).subscribe({
        next: (state) => {
          if (state.available) {
            this.landMedia(id, state.content_type);
            this.stopWatching(id);
            this.setAsking(id, false);
          } else if (state.fetch === 'failed') {
            // Back to an offer the reader can ask again.
            this.stopWatching(id);
            this.setAsking(id, false);
            this.setFetchState(id, 'failed');
          }
        },
        error: () => {
          this.stopWatching(id);
          this.setAsking(id, false);
        },
      });
    }, MEDIA_WATCH_INTERVAL_MS);
    this.watching.set(id, tick);
  }

  private stopWatching(id: string): void {
    const tick = this.watching.get(id);
    if (tick !== undefined) window.clearInterval(tick);
    this.watching.delete(id);
  }

  /** The file arrived: flip the attachment in the model so it draws. */
  private landMedia(id: string, contentType: string | null): void {
    this.messages.update((cur) =>
      cur.map((m) => ({
        ...m,
        attachments: m.attachments.map((a) =>
          a.id === id
            ? {
                ...a,
                available: true,
                fetch: null,
                content_type: contentType ?? a.content_type,
                is_image: (contentType ?? a.content_type ?? '').startsWith('image/'),
              }
            : a,
        ),
      })),
    );
  }

  private setFetchState(id: string, fetch: Attachment['fetch']): void {
    this.messages.update((cur) =>
      cur.map((m) => ({
        ...m,
        attachments: m.attachments.map((a) => (a.id === id ? { ...a, fetch } : a)),
      })),
    );
  }

  /** A size for something not yet fetched. */
  protected mib(bytes: number): string {
    const mib = bytes / (1024 * 1024);
    return mib >= 10 ? `${Math.round(mib)} MB` : `${mib.toFixed(1)} MB`;
  }

  /** Watch fetches already in flight when a page arrives, including ones asked
   *  for earlier or by someone else. */
  private watchInFlight(msgs: Message[]): void {
    for (const m of msgs) {
      for (const a of m.attachments) {
        if (a.fetch === 'wanted') this.watchMedia(a.id);
      }
    }
  }

  /** A reader asked for a link's picture; the answer comes back on this request. */
  protected requestLinkImage(offer: LinkOffer): void {
    this.setAsking(offer.id, true);
    this.api.requestLinkImage(offer.id).subscribe({
      next: (state) => {
        this.setAsking(offer.id, false);
        if (state.state === 'ok' && state.content_type) {
          this.landLinkImage(offer.id, state.content_type);
        } else {
          // Not a picture, or unreachable: the control goes.
          this.dropOffer(offer.id);
        }
      },
      error: () => {
        this.setAsking(offer.id, false);
        this.dropOffer(offer.id);
      },
    });
  }

  private setAsking(id: string, on: boolean): void {
    this.asking.update((cur) => {
      const next = new Set(cur);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });
  }

  /** Swap the offer for the picture, in place. */
  private landLinkImage(id: string, contentType: string): void {
    this.messages.update((cur) =>
      cur.map((m) => {
        const offer = m.link_offers.find((o) => o.id === id);
        if (!offer) return m;
        return {
          ...m,
          link_offers: m.link_offers.filter((o) => o.id !== id),
          link_images: [...m.link_images, { url: offer.url, id, content_type: contentType }],
        };
      }),
    );
  }

  private dropOffer(id: string): void {
    this.messages.update((cur) =>
      cur.map((m) =>
        m.link_offers.some((o) => o.id === id)
          ? { ...m, link_offers: m.link_offers.filter((o) => o.id !== id) }
          : m,
      ),
    );
  }

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

  private fetchOlder(): void {
    const o = this.origin();
    const i = this.id();
    if (o == null || i == null || !this.hasMore() || this.cursor == null || this.loadingOlder()) return;
    this.loadingOlder.set(true);
    this.api.messages(o, i, this.cursor, PAGE).subscribe({
      next: (page) => {
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
      error: () => this.loadingOlder.set(false),
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
    this.api.messages(o, i, this.newerCursor, PAGE, 'newer').subscribe({
      next: (page) => {
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
      error: () => this.loadingNewer.set(false),
    });
  }

  // ---- ?from (scroll-position restore) -----------------------------------

  private scheduleFromParam(): void {
    if (this.fromTimer) clearTimeout(this.fromTimer);
    this.fromTimer = setTimeout(() => this.commitFromParam(), 300);
  }

  commitFromParam(): void {
    // The reader has moved; the landing marker and `?at` go.
    this.landedId.set(null);
    const o = this.origin();
    const i = this.id();
    if (o == null || i == null) return;
    const from = this.win.atBottom() ? null : this.win.topAnchor()?.id;
    const ts = from ? this.messages().find((m) => m.id === from)?.ts : null;
    void this.router.navigate([], {
      relativeTo: this.route,
      queryParams: { from: ts != null ? String(ts) : null, at: null },
      queryParamsHandling: 'merge',
      replaceUrl: true,
    });
  }
}
