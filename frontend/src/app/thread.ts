import { ApplicationRef, Component, DestroyRef, ElementRef, computed, effect, inject, input, signal, viewChild } from '@angular/core';
import { DatePipe } from '@angular/common';
import { ActivatedRoute, Router } from '@angular/router';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';

import { firstValueFrom } from 'rxjs';

import { attachmentName, attachmentNoun } from './attachment';
import { chatLogHtml, formatChatLog } from './copy-log';
import { PAGE, ThreadWindow } from './thread-window';
import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
import { Conversation, Message, Origin } from './models';

/** How often an open, visible thread asks whether anything is newer.
 *
 *  Bounded below by how fast the ARCHIVE learns, not by what feels responsive:
 *  polling faster than the importer runs only adds requests that answer "no".
 *  Five seconds is under the cadence of everything upstream and cheap — the
 *  query is indexed on `(conversation_id, sent_at)` and returns one page. */
const POLL_MS = 5000;

@Component({
  selector: 'app-thread',
  templateUrl: './thread.html',
  styleUrl: './thread.scss',
  // The host IS the scroll container (class `thread`): the sticky head + day
  // headers pin against it, and it's where we read/adjust scrollTop.
  // `copy` is on the host because it BUBBLES from wherever the selection is —
  // the browser does the selecting, we only rewrite what leaves.
  host: { class: 'thread', '(scroll)': 'onScroll()', '(copy)': 'onCopy($event)' },
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

  // For the template. Naming an attachment is shared with the clipboard —
  // see attachment.ts.
  protected readonly attachmentName = attachmentName;
  protected readonly attachmentNoun = attachmentNoun;

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
  // dev-lint: allow-component-list — `messages` is an infinite-scroll pagination
  // buffer, not a retained catalog; a thread re-fetches fresh on entry by design.
  readonly messages = signal<Message[]>([]);

  /** Which of them are in the DOM, and the scrolling that decides — see
   *  thread-window.ts. It measures and moves the viewport; everything about
   *  where messages COME FROM stays here. */
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
  readonly hasMore = signal(false);
  readonly threadError = signal(false);
  private cursor: string | null = null;

  private fromTimer: ReturnType<typeof setTimeout> | null = null;



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

    const poll = setInterval(() => void this.pollNewer(), POLL_MS);
    // ⚠ Cleared with the component. An interval outlives its component
    // otherwise, and every visit to a thread would leave another one running —
    // the request rate climbing with no screen left to show the answers on.
    inject(DestroyRef).onDestroy(() => clearInterval(poll));
  }

  private resetState(): void {
    this.messages.set([]);
    this.win.reset();
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
      this.win.withScrollLock(() => {
        if (from != null) this.win.scrollToTs(from);
        else this.win.scrollToBottom();
      });
      this.win.trimToWindow();
      // Opened at the latest message: hold the bottom as lazy images load in.
      if (from == null) this.win.keepPinnedToBottom();
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

  /**
   * Replace the clipboard's text with an irssi-style log when the selection
   * covers more than one message.
   *
   * Everything hard here is the browser's: it owns the selection, the touch
   * handles, and every route to a copy — ⌘C, right-click, Android's key command
   * and its selection action bar all arrive as this one event. (⌘C is covered by
   * e2e/copy.spec.ts; the Android pair was confirmed by hand on the Pixel 9
   * against the live app, 2026-08-16.) We supply the format, and both flavours
   * at once — `text/plain` for a terminal or editor, `text/html` for Slack or
   * mail — so each target picks rather than us guessing.
   *
   * ⚠ **Below two messages we do nothing.** Selecting a phrase inside one
   * message and being handed a timestamped log line is a surprise; attribution
   * is what the user wants at two, and only then. Returning without
   * `preventDefault` leaves the browser's own copy intact.
   */
  onCopy(e: ClipboardEvent): void {
    const data = e.clipboardData;
    if (data == null) return;
    const picked = this.selectedMessages();
    if (picked.length < 2) return;
    const text = formatChatLog(picked);
    data.setData('text/plain', text);
    data.setData('text/html', chatLogHtml(text));
    e.preventDefault();
  }

  /** Which rendered messages the selection touches, in thread order.
   *
   *  The DOM answers only WHICH — `data-id` on each bubble — and the model
   *  answers what they say, so the copied log cannot drift with the template.
   *
   *  ⚠ Only what is RENDERED can be selected: the window collapses the rest out
   *  of the DOM behind spacers, so a select-all copies the window, not the
   *  conversation. That matches what the user could see and scroll through. */
  private selectedMessages(): Message[] {
    const el = this.messagesEl()?.nativeElement;
    const sel = document.getSelection();
    if (el == null || sel == null || sel.isCollapsed) return [];
    // Firefox allows several ranges in one selection; everything else gives
    // exactly one. Taking them all costs a loop.
    const ranges = Array.from({ length: sel.rangeCount }, (_, i) => sel.getRangeAt(i));
    const ids = new Set<string>();
    for (const node of el.querySelectorAll<HTMLElement>('.msg[data-id]')) {
      // ⚠ `Range.intersectsNode`, NOT `Selection.containsNode(node, true)`.
      // "Contains" asks whether the bubble sits inside the selection, so a
      // selection inside ONE bubble reports that bubble as unselected — the
      // exact case the two-message threshold turns on. Intersects asks whether
      // they overlap, which is the question: half a bubble selected is that
      // message selected, because a log line is whole or it is a misquote.
      // A switch back would be caught by e2e/copy.spec.ts and NOT by the vitest
      // specs — jsdom gets both APIs wrong.
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

  /** Ask whether anything is newer, and merge it in.
   *
   *  ⚠ **Without this, an instant archive is invisible.** Measured before it
   *  existed: the app showed a message count three behind the database for an
   *  action the user had just taken himself. Everything upstream — the hourly
   *  import, the send path's immediate echo — lands in a page that was fetched
   *  once and never asked again.
   *
   *  Reuses the ordinary newest-page endpoint rather than adding a `?since`
   *  one. The query is indexed on `(conversation_id, sent_at)` and returns one
   *  page, so the saving would be bytes on a local network, against a second
   *  code path that could disagree with the first about ordering or filtering.
   *
   *  Silent on failure by design: the thread on screen is still correct, the
   *  next tick tries again, and an error banner for a background refetch would
   *  be alarming out of proportion to a dropped packet. */
  async pollNewer(): Promise<void> {
    const o = this.origin();
    const i = this.id();
    if (o == null || i == null) return;
    // Don't compete with a load that is already fetching this same page, and
    // don't poll a screen nobody is looking at — a backgrounded phone app would
    // otherwise keep asking forever.
    if (this.polling || this.loadingThread() || this.loadingOlder() || this.sending()) return;
    if (document.visibilityState !== 'visible') return;
    if (this.messages().length === 0) return;

    this.polling = true;
    try {
      const page = await firstValueFrom(this.api.messages(o, i, undefined, PAGE));
      // The user may have moved to another conversation while this was in
      // flight; merging then would put one thread's messages in another.
      if (this.origin() !== o || this.id() !== i) return;

      const held = this.messages();
      const known = new Set(held.map((m) => m.id));
      const fresh = page.messages.filter((m) => !known.has(m.id));
      if (fresh.length === 0) return;

      // ⚠ NOT ONE MESSAGE OF THE NEWEST PAGE IS KNOWN, so more than a page has
      // arrived since the last poll and there is a gap between what is held and
      // what came back. Merging would leave a hole in the middle of the thread
      // that no amount of scrolling could fill, and nothing would report it.
      if (fresh.length === page.messages.length) {
        await this.loadThread(o, i);
        return;
      }

      const wasAtBottom = this.win.atBottom();
      // Sorted rather than appended: an import can write a line with an older
      // timestamp (a backfilled day landing late), and appending it would put
      // the thread out of order rather than leave it in the past where it
      // belongs.
      this.messages.set(
        [...held, ...fresh].sort((a, b) => a.ts - b.ts || a.id.localeCompare(b.id)),
      );
      this.appRef.tick();
      // Follow the conversation only if the user was already at the end of it.
      // Yanking someone reading history down to the newest line is the single
      // most annoying thing a chat app does.
      if (wasAtBottom) this.win.withScrollLock(() => this.win.scrollToBottom());
      this.win.trimToWindow();
    } catch {
      // See the doc comment: a failed poll is not an error state.
    } finally {
      this.polling = false;
    }
  }

  // ---- sending (IRC only) -------------------------------------------------

  /** Only IRC has a live client behind it. Signal and Google Chat are archives
   *  of conversations held elsewhere, so there is nothing here to send with —
   *  showing a box that always failed would be worse than showing none. */
  readonly canSend = computed(() => this.origin() === 'irc' && this.routed());
  readonly draft = signal('');
  readonly sending = signal(false);
  /** The far side's refusal, shown as-is: it is the only place that knows why,
   *  and "could not send" would hide the usual reason (no tab open there). */
  readonly sendError = signal<string | null>(null);

  async send(): Promise<void> {
    const o = this.origin();
    const i = this.id();
    const text = this.draft().trim();
    if (o == null || i == null || !text || this.sending()) return;

    this.sending.set(true);
    this.sendError.set(null);
    try {
      const res = await firstValueFrom(this.api.send(o, i, text));
      if (!res.sent) {
        this.sendError.set(res.error ?? 'Not sent.');
        return;
      }
      // Cleared only once irssi says it went: a failed send that empties the box
      // loses what was typed, and retyping it is the worst moment to do so.
      this.draft.set('');
      if (res.archived) {
        // The backend wrote what irssi logged, so a reload shows the real line
        // rather than an optimistic copy of what we asked for.
        await this.loadThread(o, i);
      } else {
        // Sent, but not yet in the archive — the hourly import will bring it.
        // Saying so is better than silently showing a conversation that appears
        // not to contain the message just sent.
        this.sendError.set('Sent. It will appear here after the next import.');
      }
    } catch {
      this.sendError.set('Could not reach the server.');
    } finally {
      this.sending.set(false);
    }
  }

  /** Enter sends; Shift+Enter is a newline — except that a newline cannot be
   *  sent at all (IRC lines are newline-delimited and the far side refuses one),
   *  so the box is single-line and this only stops the form feeling odd. */
  onComposerKey(e: KeyboardEvent): void {
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

  // ---- scrolling ----------------------------------------------------------

  /** The scroll handler. The window engine decides what to reveal or collapse;
   *  the one thing it cannot do is fetch, because it does not know where
   *  messages come from. */
  onScroll(): void {
    if (this.win.busy || this.loadingThread() || this.threadError() || !this.routed()) return;
    if (this.win.step().needOlder) this.fetchOlder();
    this.scheduleFromParam();
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

  // ---- ?from (scroll-position restore) -----------------------------------

  private scheduleFromParam(): void {
    if (this.fromTimer) clearTimeout(this.fromTimer);
    this.fromTimer = setTimeout(() => this.commitFromParam(), 300);
  }

  private commitFromParam(): void {
    const o = this.origin();
    const i = this.id();
    if (o == null || i == null) return;
    const from = this.win.atBottom() ? null : this.win.topAnchor()?.id;
    const ts = from ? this.messages().find((m) => m.id === from)?.ts : null;
    void this.router.navigate([], {
      relativeTo: this.route,
      queryParams: { from: ts != null ? String(ts) : null },
      queryParamsHandling: 'merge',
      replaceUrl: true,
    });
  }
}
