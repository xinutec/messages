import { ComponentRef, provideZonelessChangeDetection } from '@angular/core';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { ActivatedRoute, Router, convertToParamMap, provideRouter } from '@angular/router';
import { BehaviorSubject, Observable, of } from 'rxjs';
import { describe, expect, it, vi } from 'vitest';

import { Thread } from './thread';
import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
import { Message, MessagesPage } from './models';
import { MAX_RESTORE_PAGES } from './thread-window';

function msg(id: string, ts: number): Message {
  return { id, ts, sender: 's', is_outgoing: false, kind: 'message', body: 'b', deleted: false, edited: false, reactions: [], attachments: [] };
}

function makeApi() {
  return {
    me: vi.fn(() => of({ user_id: 'u1', display_name: 'Test User' })),
    conversations: vi.fn(() => of([])),
    messages: vi.fn(() => of({ messages: [msg('1', 100)], has_more: false, next_cursor: null, prev_cursor: null } as MessagesPage)),
    search: vi.fn(() => of([])),
    logout: vi.fn(() => of({})),
    send: vi.fn(() => of({ sent: true, error: null, archived: true })),
  } as unknown as MessagesApi;
}

function setup(): { thread: Thread; ref: ComponentRef<Thread>; fixture: ComponentFixture<Thread>; router: Router } {
  TestBed.configureTestingModule({
    providers: [
      provideZonelessChangeDetection(),
      provideRouter([]),
      { provide: MessagesApi, useValue: makeApi() },
    ],
  });
  // createComponent (not `new Thread()`): the component injects ElementRef, which
  // only exists for a real component instance.
  const fixture = TestBed.createComponent(Thread);
  return { thread: fixture.componentInstance, ref: fixture.componentRef, fixture, router: TestBed.inject(Router) };
}

function page(
  messages: Message[],
  has_more = false,
  next_cursor: string | null = null,
  prev_cursor: string | null = null,
): Observable<MessagesPage> {
  return of({ messages, has_more, next_cursor, prev_cursor });
}

/** A thread routed to an IRC conversation and settled on `held`.
 *
 *  ⚠ The initial load has to be let finish. Routing the inputs starts it, and
 *  `pollNewer` deliberately declines to run while a load is in flight — so a
 *  test that set `messages` by hand and polled immediately measured the guard
 *  rather than the merge, and passed for the wrong reason. */
async function opened(held: Message[]): Promise<{ thread: Thread; api: { messages: ReturnType<typeof vi.fn> } }> {
  const { thread, ref, fixture } = setup();
  const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
  api.messages.mockReturnValue(page(held));
  ref.setInput('origin', 'irc');
  ref.setInput('id', '7');
  fixture.detectChanges();
  for (let i = 0; i < 20 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
  expect(thread.messages().map((m) => m.id)).toEqual(held.map((m) => m.id));
  return { thread, api };
}

describe('Thread', () => {
  it('dayGroups buckets consecutive rendered messages by calendar day', () => {
    const { thread } = setup();
    const d1 = new Date(2026, 5, 1, 9, 0, 0).getTime();
    const d1b = new Date(2026, 5, 1, 18, 0, 0).getTime();
    const d2 = new Date(2026, 5, 2, 9, 0, 0).getTime();
    // With nothing collapsed, the rendered window is the whole retained list.
    thread.messages.set([msg('a', d1), msg('b', d1b), msg('c', d2)]);
    const groups = thread.dayGroups();
    expect(groups.length).toBe(2);
    expect(groups[0].items.map((m) => m.id)).toEqual(['a', 'b']);
    expect(groups[1].items.map((m) => m.id)).toEqual(['c']);
  });

  it('rendered window equals retained messages when nothing is collapsed', () => {
    const { thread } = setup();
    thread.messages.set([msg('a', 1), msg('b', 2), msg('c', 3)]);
    expect(thread.renderCount()).toBe(3);
    expect(thread.rendered().map((m) => m.id)).toEqual(['a', 'b', 'c']);
    expect(thread.topSpacer()).toBe(0);
    expect(thread.bottomSpacer()).toBe(0);
  });

  it('back returns to the list route, dropping the paged depth', () => {
    const { thread, router } = setup();
    const nav = vi.spyOn(router, 'navigate').mockResolvedValue(true);
    thread.back();
    // origin filter preserved (merge); from cleared.
    expect(nav).toHaveBeenCalledWith(['/'], expect.objectContaining({ queryParams: { from: null }, queryParamsHandling: 'merge' }));
  });
  it('offers a composer for IRC only — the other origins have no live client', () => {
    const { thread, ref, fixture } = setup();
    ref.setInput('origin', 'signal');
    ref.setInput('id', 'dm:a');
    fixture.detectChanges();
    expect(thread.canSend()).toBe(false);

    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();
    expect(thread.canSend()).toBe(true);
  });

  it('keeps the draft when the send is refused, and clears it only once it went', async () => {
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as { send: ReturnType<typeof vi.fn> };
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();

    // The far side refuses — most often because irssi has no tab open with
    // that target, which is what decides.
    api.send.mockReturnValueOnce(of({ sent: false, error: 'refused: no conversation open with that target', archived: false }));
    thread.draft.set('please keep me');
    await thread.send();
    expect(thread.draft()).toBe('please keep me');
    expect(thread.sendError()).toContain('no conversation open');

    // ⚠ The box is emptied only on a real send. Clearing on failure loses what
    // was typed at the exact moment the person has to type it again.
    api.send.mockReturnValueOnce(of({ sent: true, error: null, archived: true }));
    await thread.send();
    expect(thread.draft()).toBe('');
  });

  it('says so when a message went but is not in the archive yet', async () => {
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as { send: ReturnType<typeof vi.fn> };
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();

    // Sent, but irssi could not find the echo in its log — the hourly import
    // will bring it. Silence would show a conversation that appears not to
    // contain the message just sent.
    api.send.mockReturnValueOnce(of({ sent: true, error: null, archived: false }));
    thread.draft.set('gone, but not seen');
    await thread.send();
    expect(thread.draft()).toBe('');
    expect(thread.sendError()).toContain('after the next import');
  });

  it('merges messages that arrived since the page was loaded', async () => {
    const { thread, api } = await opened([msg('1', 100), msg('2', 200)]);
    // The newest page overlaps what is held and carries one more.
    api.messages.mockReturnValueOnce(page([msg('1', 100), msg('2', 200), msg('3', 300)]));
    await thread.pollNewer();
    expect(thread.messages().map((m) => m.id)).toEqual(['1', '2', '3']);
  });

  it('puts a late-arriving older line in its place rather than at the end', async () => {
    const { thread, api } = await opened([msg('1', 100), msg('3', 300)]);
    // An import can write a line with an older timestamp — a backfilled day
    // landing after a newer one. Appending it would show it as the latest thing
    // said, which is worse than not showing it at all.
    api.messages.mockReturnValueOnce(page([msg('1', 100), msg('2', 200), msg('3', 300)]));
    await thread.pollNewer();
    expect(thread.messages().map((m) => m.id)).toEqual(['1', '2', '3']);
  });

  it('reloads instead of merging when the newest page overlaps nothing held', async () => {
    const { thread, api } = await opened([msg('1', 100)]);
    // ⚠ Not one message of the page is known, so more arrived than a page holds
    // and there is a gap between '1' and what came back. Merging would leave a
    // hole in the middle of the thread that scrolling could never fill and
    // nothing would report — so this must re-load the thread instead.
    api.messages.mockReturnValue(page([msg('8', 800), msg('9', 900)], true, 'c'));
    await thread.pollNewer();
    expect(thread.messages().map((m) => m.id)).toEqual(['8', '9']);
    expect(thread.hasMore()).toBe(true);
  });

  it('does not poll a screen nobody is looking at', async () => {
    const { thread, api } = await opened([msg('1', 100)]);
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    api.messages.mockClear();
    await thread.pollNewer();
    // A backgrounded phone app that keeps asking is a battery cost with no
    // screen left to show the answers on.
    expect(api.messages).not.toHaveBeenCalled();
    visibility.mockRestore();
  });

  it('does not send an empty or whitespace-only draft', async () => {
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as { send: ReturnType<typeof vi.fn> };
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();

    thread.draft.set('   ');
    await thread.send();
    expect(api.send).not.toHaveBeenCalled();
  });
});

// ---- copying a selection as a chat log --------------------------------------
//
// The formatting itself is covered by copy-log.spec.ts, which needs no DOM.
// What these check is the half that only a document can answer: that a real
// Range across real bubbles picks the right messages, and that the two-message
// threshold holds.

/** Three rendered messages over two days, attached to the document so a real
 *  Range can be laid across them. */
async function threeRendered(): Promise<ComponentFixture<Thread>> {
  const { thread, ref, fixture } = setup();
  const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
  api.messages.mockReturnValue(
    page([
      { ...msg('a', new Date(2026, 7, 13, 14, 32).getTime()), sender: 'pippijn', body: 'hello there' },
      { ...msg('b', new Date(2026, 7, 13, 14, 33).getTime()), sender: 'simon', body: 'hi' },
      { ...msg('c', new Date(2026, 7, 14, 9, 5).getTime()), sender: 'simon', body: 'morning' },
    ]),
  );
  ref.setInput('origin', 'irc');
  ref.setInput('id', '7');
  fixture.detectChanges();
  for (let i = 0; i < 20 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
  fixture.detectChanges();
  return fixture;
}

/** Lay a selection from one offset in one message's body to another, as a drag
 *  would. Same-id, same-offsets is a selection inside a single message. */
function select(fixture: ComponentFixture<Thread>, from: [string, number], to: [string, number]): void {
  const root = fixture.nativeElement as HTMLElement;
  const text = (id: string): ChildNode =>
    root.querySelector(`.msg[data-id="${id}"] .body`)!.firstChild!;
  const range = document.createRange();
  range.setStart(text(from[0]), from[1]);
  range.setEnd(text(to[0]), to[1]);
  const sel = document.getSelection()!;
  sel.removeAllRanges();
  sel.addRange(range);
}

/** Fire the copy the platform fires, and report what reached the clipboard. */
function fireCopy(fixture: ComponentFixture<Thread>): { written: Map<string, string>; prevented: boolean } {
  const written = new Map<string, string>();
  const ev = new Event('copy', { bubbles: true, cancelable: true });
  (ev as unknown as { clipboardData: unknown }).clipboardData = {
    setData: (type: string, value: string) => written.set(type, value),
  };
  (fixture.nativeElement as HTMLElement).querySelector('.messages')!.dispatchEvent(ev);
  return { written, prevented: ev.defaultPrevented };
}

describe('Thread rendering', () => {
  it('draws an action with its star, which the backend no longer sends', async () => {
    // ⚠ The star moved from `archive.rs` to here when `kind` reached the API.
    // On screen nothing was supposed to change, and nothing else would say so:
    // the body now arrives as the words alone, so a template that forgot the
    // star would render `waves` and look like ordinary speech.
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
    api.messages.mockReturnValue(
      page([
        { ...msg('m', 100), sender: 'alice', body: 'hello' },
        { ...msg('a', 200), sender: 'alice', kind: 'action', body: 'waves' },
      ]),
    );
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();
    for (let i = 0; i < 20 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    fixture.detectChanges();
    const bodies = [...(fixture.nativeElement as HTMLElement).querySelectorAll('.msg .body')].map(
      (e) => e.textContent,
    );
    expect(bodies).toEqual(['hello', '* waves']);
  });
});

describe('Thread copy', () => {
  it('copies a multi-message selection as an irssi log', async () => {
    const fixture = await threeRendered();
    select(fixture, ['a', 0], ['c', 7]);
    const { written, prevented } = fireCopy(fixture);
    expect(prevented).toBe(true);
    expect(written.get('text/plain')).toBe(
      '--- Day changed Thu Aug 13 2026\n' +
        '14:32 <pippijn> hello there\n' +
        '14:33 <simon> hi\n' +
        '--- Day changed Fri Aug 14 2026\n' +
        '09:05 <simon> morning',
    );
  });

  it('offers the same log as rich text, so the paste target can choose', async () => {
    const fixture = await threeRendered();
    select(fixture, ['a', 0], ['b', 2]);
    const html = fireCopy(fixture).written.get('text/html')!;
    expect(html.startsWith('<pre>')).toBe(true);
    expect(html).toContain('&lt;pippijn&gt;');
  });

  it('takes a message the selection only clips, whole', async () => {
    // Half a bubble selected is that message selected: a log line is whole or
    // it is a misquote.
    const fixture = await threeRendered();
    select(fixture, ['a', 6], ['b', 1]);
    expect(fireCopy(fixture).written.get('text/plain')).toBe(
      '--- Day changed Thu Aug 13 2026\n14:32 <pippijn> hello there\n14:33 <simon> hi',
    );
  });

  it('leaves a selection inside one message to the browser', async () => {
    // Picking a phrase out of a sentence and being handed a timestamped log
    // line is a surprise; attribution starts mattering at two.
    const fixture = await threeRendered();
    select(fixture, ['a', 0], ['a', 5]);
    const { written, prevented } = fireCopy(fixture);
    expect(prevented).toBe(false);
    expect(written.size).toBe(0);
  });

  it('leaves a copy with no selection alone', async () => {
    const fixture = await threeRendered();
    document.getSelection()!.removeAllRanges();
    expect(fireCopy(fixture).prevented).toBe(false);
  });

  /** ⚠ The screen and the clipboard must call an attachment the same thing, and
   *  they did not. Each composed the label itself — the template with a
   *  `file_name || content_type || 'attachment'` chain, `copy-log.ts` with a
   *  rule about when the type is worth printing — so a stored-less image with no
   *  filename read `image/jpeg (not stored)` on screen and `[image (not stored)]`
   *  in the paste. Naming lives in `attachment.ts` now; this is the case that
   *  told the two copies apart, so it is the case that has to stay pinned. */
  it('calls a nameless unavailable image what it is, as the clipboard does', async () => {
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
    api.messages.mockReturnValue(
      page([
        {
          ...msg('a', new Date(2026, 7, 13, 14, 32).getTime()),
          attachments: [
            { id: 'x', content_type: 'image/jpeg', file_name: null, size: null, available: false, is_image: true },
          ],
        },
      ]),
    );
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();
    for (let i = 0; i < 20 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    fixture.detectChanges();

    const text = (fixture.nativeElement as HTMLElement).querySelector('.attach')?.textContent ?? '';
    expect(text).toContain('image (not stored)');
    expect(text).not.toContain('image/jpeg');
  });
});

/** A thread holding one ordinary message and one deleted one, rendered.
 *  `withImage` gives the deleted message an available image instead of a body —
 *  the attachment-only shape, which renders no `.body` div at all. */
async function withDeleted(withImage = false): Promise<ComponentFixture<Thread>> {
  const { thread, ref, fixture } = setup();
  const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
  const del: Message = withImage
    ? {
        ...msg('d', new Date(2026, 7, 13, 14, 33).getTime()),
        body: null,
        deleted: true,
        attachments: [
          { id: 'i1', content_type: 'image/jpeg', file_name: 'x.jpg', size: 10, available: true, is_image: true },
        ],
      }
    : { ...msg('d', new Date(2026, 7, 13, 14, 33).getTime()), sender: 'simon', body: 'the retracted words', deleted: true };
  api.messages.mockReturnValue(
    page([{ ...msg('a', new Date(2026, 7, 13, 14, 32).getTime()), sender: 'pippijn', body: 'hello there' }, del]),
  );
  ref.setInput('origin', 'irc');
  ref.setInput('id', '7');
  fixture.detectChanges();
  for (let i = 0; i < 20 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
  fixture.detectChanges();
  return fixture;
}

const revealBtn = (f: ComponentFixture<Thread>): HTMLButtonElement | null =>
  (f.nativeElement as HTMLElement).querySelector('.msg[data-id="d"] .reveal');

describe('Thread deleted messages', () => {
  it('does not render a deleted message\'s text until it is revealed', async () => {
    const f = await withDeleted();
    const bubble = (f.nativeElement as HTMLElement).querySelector('.msg[data-id="d"]')!;
    expect(bubble.textContent).toContain('(deleted)');
    expect(bubble.textContent).not.toContain('the retracted words');

    revealBtn(f)!.click();
    f.detectChanges();
    expect(bubble.textContent).toContain('the retracted words');
  });

  /** ⚠ THE HALF THAT WAS NEVER HIDDEN. The attachment loop was not gated on
   *  `m.deleted`, so a deleted message drew its pictures in full while its words
   *  read `(deleted)`. Measured against the live archive on 2026-09-03: 17 stored
   *  images on 3 deleted messages, 16 with loaded pixels on screen.
   *
   *  ⚠ And this is the shape that hides from a careless check: with no body,
   *  `@if (m.body)` renders no `.body` div, so a DOM probe keyed on
   *  `.body.deleted` reports zero of exactly these. Key on the bubble. */
  it('does not render a deleted message\'s images until it is revealed', async () => {
    const f = await withDeleted(true);
    const bubble = (f.nativeElement as HTMLElement).querySelector('.msg[data-id="d"]')!;
    expect(bubble.querySelectorAll('img').length).toBe(0);

    revealBtn(f)!.click();
    f.detectChanges();
    expect(bubble.querySelectorAll('img').length).toBe(1);
  });

  it('re-hides on a second click', async () => {
    const f = await withDeleted();
    revealBtn(f)!.click();
    f.detectChanges();
    const bubble = (f.nativeElement as HTMLElement).querySelector('.msg[data-id="d"]')!;
    expect(bubble.textContent).toContain('the retracted words');
    bubble.querySelector<HTMLButtonElement>('.rehide')!.click();
    f.detectChanges();
    expect(bubble.textContent).not.toContain('the retracted words');
  });

  /** Revealing is a decision about THIS screen. The clipboard is a different
   *  place with a different audience, so it keeps saying `(deleted)` — and it
   *  does because the log is built from the model, which the reveal never
   *  touches. This test exists to keep it that way. */
  it('still copies a revealed message as (deleted)', async () => {
    const f = await withDeleted();
    revealBtn(f)!.click();
    f.detectChanges();
    select(f, ['a', 0], ['d', 5]);
    const { written } = fireCopy(f);
    expect(written.get('text/plain')).toContain('(deleted)');
    expect(written.get('text/plain')).not.toContain('the retracted words');
  });
});

describe('Thread copy — saying what was left out', () => {
  /** The thread as three rendered messages out of a conversation of `total`. */
  async function opened3(total: number): Promise<ComponentFixture<Thread>> {
    const f = await threeRendered();
    TestBed.inject(MessagesStore).conversations.set([
      { origin: 'irc', id: '7', name: '#linux', kind: 'group', network: 'xinutec', message_count: total, last_ts: 1 },
    ]);
    f.detectChanges();
    return f;
  }

  /** ⚠ A select-all that quietly returns a fraction is the whole bug. The DOM
   *  holds a bounded window, so "everything" means "everything loaded". */
  it('a whole-window copy in a truncated thread says what was left out', async () => {
    const f = await opened3(401794);
    select(f, ['a', 0], ['c', 7]);
    expect(fireCopy(f).written.get('text/plain')).toContain('copied 3 of 401794 messages');
  });

  /** ⚠ And it must NOT fire for an ordinary selection. Picking two lines out of
   *  a long conversation is not a truncated copy; it is a quote, and appending
   *  "copied 2 of 401794" to it would be noise on every paste. */
  it('a deliberate two-message copy says nothing about the rest', async () => {
    const f = await opened3(401794);
    select(f, ['a', 0], ['b', 2]);
    const out = fireCopy(f).written.get('text/plain');
    expect(out).toContain('hello there');
    expect(out).not.toContain('not loaded');
  });

  it('says nothing when the window already holds the whole conversation', async () => {
    const f = await opened3(3);
    select(f, ['a', 0], ['c', 7]);
    expect(fireCopy(f).written.get('text/plain')).not.toContain('not loaded');
  });
});

describe('Thread composer — typing with an IME', () => {
  async function composer(): Promise<{ thread: Thread; api: { send: ReturnType<typeof vi.fn> } }> {
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as {
      messages: ReturnType<typeof vi.fn>;
      send: ReturnType<typeof vi.fn>;
    };
    api.messages.mockReturnValue(page([msg('a', 1000)]));
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();
    for (let i = 0; i < 20 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    return { thread, api };
  }

  const enter = (over: KeyboardEventInit = {}): KeyboardEvent =>
    new KeyboardEvent('keydown', { key: 'Enter', cancelable: true, ...over });

  /** ⚠ **THE ANDROID ONE.** While an IME has a composition in flight — a word
   *  still underlined under predictive text, a swipe-typed word, anything in a
   *  language that composes — Enter means "accept the candidate", not "send".
   *  The browser says so with `isComposing`, and a handler that does not ask
   *  sends half a word the moment the user reaches for their own keyboard's
   *  autocomplete. Nothing about this is visible on a desktop with a hardware
   *  keyboard, which is where it was written. */
  it('leaves a composing Enter to the IME rather than answering it', async () => {
    const { thread, api } = await composer();
    thread.draft.set('hello wor');
    const e = enter({ isComposing: true });
    thread.onComposerKey(e);
    expect(api.send).not.toHaveBeenCalled();
    // The key is left to the IME, which needs it to commit the candidate.
    expect(e.defaultPrevented).toBe(false);
  });

  /** ⚠ **AND THAT IS NOT ENOUGH, WHICH THIS LAYER CANNOT SEE.** The test above
   *  passed against a version that still sent the composed word: not answering
   *  the key leaves the browser to submit the `<form>` implicitly, and `send`
   *  was reached that way instead. Calling the handler in isolation never
   *  involves a form, so jsdom reported a fix that a real browser refuted —
   *  `e2e/ui-pages.spec.ts` drives an actual composition through CDP and is the
   *  evidence. What IS worth pinning here is the backstop it led to: `send`
   *  refuses on its own, whichever route reached it. */
  it('refuses to send while composing, whatever route reached send', async () => {
    const { thread, api } = await composer();
    thread.draft.set('hello wor');
    thread.composing.set(true);
    await thread.send();
    expect(api.send).not.toHaveBeenCalled();

    thread.composing.set(false);
    await thread.send();
    expect(api.send).toHaveBeenCalledTimes(1);
  });

  it('sends on a plain Enter, as before', async () => {
    const { thread, api } = await composer();
    thread.draft.set('hello world');
    const e = enter();
    thread.onComposerKey(e);
    expect(api.send).toHaveBeenCalledTimes(1);
    expect(e.defaultPrevented).toBe(true);
  });
});
describe('restoring a saved scroll depth', () => {
  /** An `?from` older than anything the API will ever return, with the API
   *  always claiming another page. That is a stale bookmark: the loop cannot
   *  reach the timestamp and nothing else stops it. */
  async function openWithFrom(from: string): Promise<{ calls: number }> {
    TestBed.configureTestingModule({
      providers: [
        provideZonelessChangeDetection(),
        provideRouter([]),
        { provide: MessagesApi, useValue: makeApi() },
        {
          provide: ActivatedRoute,
          useValue: {
            snapshot: { queryParamMap: convertToParamMap({ from }) },
            queryParamMap: of(convertToParamMap({ from })),
          },
        },
      ],
    });
    const fixture = TestBed.createComponent(Thread);
    const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
    // Every page is newer than `from`, so nothing but the bound ends the loop.
    //
    // ⚠ THE SUPPLY IS FINITE ON PURPOSE, and it is not a weaker test for it.
    // Against a server that never runs out — the real shape of this — removing
    // the bound does not fail this test, it HANGS: the loop is driven by
    // resolved promises, so it starves the macrotask queue and the wait below
    // never gets a turn. Verified by ablation 2026-09-07 (no output in 240s).
    // A suite that wedges is a worse instrument than one that fails, so the
    // supply stops a few pages past the budget: the bound makes
    // MAX_RESTORE_PAGES + 1 requests, and its absence makes more and says so.
    let n = 1000;
    let left = MAX_RESTORE_PAGES + 5;
    api.messages.mockImplementation(() => page([msg(String(n--), 5000)], left-- > 0, 'c'));
    fixture.componentRef.setInput('origin', 'irc');
    fixture.componentRef.setInput('id', '7');
    fixture.detectChanges();
    const thread = fixture.componentInstance;
    for (let i = 0; i < 200 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    expect(thread.loadingThread()).toBe(false);
    return { calls: api.messages.mock.calls.length };
  }

  /** ⚠ Without the bound this does not fail, it HANGS — the loop has more pages
   *  and has not reached the timestamp, forever. A bookmark into a busy channel
   *  is the real shape of it. */
  it('stops after a bounded number of requests when `from` is unreachable', async () => {
    const { calls } = await openWithFrom('1');
    // The first page, plus at most the restore budget.
    expect(calls).toBeLessThanOrEqual(MAX_RESTORE_PAGES + 1);
    // And it did page back rather than giving up at the first turn — a bound
    // that stopped immediately would pass the line above and break restoring.
    expect(calls).toBeGreaterThan(1);
  });

  it('stops as soon as the saved depth is reached, well inside the bound', async () => {
    // `from` is at the first page's own timestamp, so the loop never runs.
    const { calls } = await openWithFrom('5000');
    expect(calls).toBe(1);
  });
});

/** Landing on a search hit — #1401.
 *
 *  Clicking a result opened the conversation at its NEWEST page while the hit
 *  itself might be years back, and nothing scrolled to it. That became
 *  load-bearing on 2026-09-04, when search started returning retracted messages
 *  with `(deleted)` in place of the snippet: the reveal lives in the thread, so
 *  the row's whole job is to deliver you to the message.
 *
 *  `?at` is the hit's opaque cursor. It means "put me here"; `?from` means "I
 *  was here". They never meaningfully coexist — `at` wins on load and
 *  `commitFromParam` writes `from` as soon as the reader scrolls. */
describe('landing on a search hit', () => {
  /** `newerHasMore` is the whole difference between the two situations a
   *  landing can be in: a hit with the conversation still running on after it,
   *  and a hit that happens to be near the end. */
  async function openAt(at: string, newerHasMore = true): Promise<{
    thread: Thread;
    calls: { cursor?: string; dir?: string }[];
  }> {
    TestBed.configureTestingModule({
      providers: [
        provideZonelessChangeDetection(),
        provideRouter([]),
        { provide: MessagesApi, useValue: makeApi() },
        {
          provide: ActivatedRoute,
          useValue: {
            snapshot: { queryParamMap: convertToParamMap({ at }) },
            queryParamMap: of(convertToParamMap({ at })),
          },
        },
      ],
    });
    const fixture = TestBed.createComponent(Thread);
    const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
    const calls: { cursor?: string; dir?: string }[] = [];
    api.messages.mockImplementation(
      (_o: unknown, _i: unknown, cursor?: string, _limit?: number, dir?: string) => {
        calls.push({ cursor, dir });
        // Older-half: the two before the hit. Newer-half: the hit and the two
        // after it, which is what makes the landing readable in both
        // directions rather than an end with nothing past it.
        // ⚠ `at`, not `newer`. A landing asks for the INCLUSIVE direction; a
        // mock that answered `newer` was standing in for a backend that
        // included the hit, which is what the author believed and not what the
        // code did — and that belief passing as a test is how the skipped-hit
        // bug reached a phone.
        return dir === 'at'
          ? page([msg('h', 5000), msg('n1', 6000), msg('n2', 7000)], newerHasMore, 'newer-c')
          : page([msg('o1', 3000), msg('o2', 4000)], true, 'older-c');
      },
    );
    fixture.componentRef.setInput('origin', 'irc');
    fixture.componentRef.setInput('id', '7');
    fixture.detectChanges();
    const thread = fixture.componentInstance;
    for (let i = 0; i < 40 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    return { thread, calls };
  }

  it('asks for the messages on BOTH sides of the hit', async () => {
    const { calls } = await openAt('5000_9');
    // Not the newest page. Opening at the end is what the bug was.
    expect(calls.every((c) => c.cursor === '5000_9')).toBe(true);
    // `at`, not `newer` — the forward half of a landing must INCLUDE the hit.
    expect(calls.map((c) => c.dir).sort()).toEqual(['at', 'older']);
  });

  it('holds the hit with context after it, not only before it', async () => {
    const { thread } = await openAt('5000_9');
    const ids = thread.messages().map((m) => m.id);
    expect(ids).toEqual(['o1', 'o2', 'h', 'n1', 'n2']);
    // ⚠ The half that matters. A landing with only older messages loaded is
    // half a conversation, and usually the half being searched for — a reply
    // is what tells you whether the message you found meant anything.
    expect(ids.indexOf('h')).toBeLessThan(ids.length - 1);
  });

  it('is not treated as being at the latest message', async () => {
    const { thread } = await openAt('5000_9');
    // A floating window has no overlap with the newest page, so `pollNewer`'s
    // gap guard would call the whole thread unfillable and reload it — landing
    // the reader back in the present within POLL_MS. Nothing about that reads
    // as a bug from the outside; the thread just leaves.
    expect(thread.floating()).toBe(true);
  });

  /** ⚠ The mirror, and it is not a formality. `floating` is what suppresses
   *  `pollNewer`, so a landing that reports it unconditionally would leave a
   *  reader who arrived at the END of a conversation never seeing another
   *  message — the poll silenced for a window that was at the present all
   *  along. The forward half running out is what says so. */
  it('a hit near the end of the conversation is at the latest message', async () => {
    const { thread } = await openAt('5000_9', false);
    expect(thread.floating()).toBe(false);
  });
});

/** ⚠ **A HIT IN THE CONVERSATION ALREADY ON SCREEN.** The reload effect keys on
 *  origin+id, and Angular reuses this component across navigations — so
 *  clicking a result from the thread you are already looking at changed only
 *  the query string, the key was equal, and nothing reloaded. The row did
 *  nothing at all, which is the same symptom #1401 was filed for and the one a
 *  fix aimed only at other conversations would leave behind.
 *
 *  Search is reachable beside an open thread on a wide screen, so this is a
 *  click somebody makes, not a contrived route. */
describe('landing again without changing conversation', () => {
  it('re-lands when only ?at changes', async () => {
    const params: Record<string, string> = { at: '5000_9' };
    // ⚠ The real `queryParamMap` stream, not a hand-called handler. A test that
    // invoked the method directly would pass with nothing subscribed to it,
    // which is the wiring this is about.
    const qp = new BehaviorSubject(convertToParamMap(params));
    TestBed.configureTestingModule({
      providers: [
        provideZonelessChangeDetection(),
        provideRouter([]),
        { provide: MessagesApi, useValue: makeApi() },
        {
          provide: ActivatedRoute,
          useValue: {
            queryParamMap: qp,
            get snapshot() {
              return { queryParamMap: qp.value };
            },
          },
        },
      ],
    });
    const fixture = TestBed.createComponent(Thread);
    const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
    let loads = 0;
    api.messages.mockImplementation((_o: unknown, _i: unknown, _c?: string, _l?: number, dir?: string) => {
      if (dir === 'at') loads++;
      return page([msg('h', 5000)], true, 'c');
    });
    fixture.componentRef.setInput('origin', 'irc');
    fixture.componentRef.setInput('id', '7');
    fixture.detectChanges();
    const thread = fixture.componentInstance;
    for (let i = 0; i < 40 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    expect(loads).toBe(1);

    // Same conversation, a different hit in it.
    qp.next(convertToParamMap({ at: '9000_11' }));
    for (let i = 0; i < 40 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    expect(loads).toBe(2);
  });
});

/** Scrolling FORWARD off a landing — the other half of #1401.
 *
 *  Before this the window could only grow backwards: the sole route by which
 *  newer messages reached a thread was `pollNewer` asking for the newest page.
 *  A reader put on a 2005 hit could scroll back for ever and not forward one
 *  line, so the hit was a dead end. */
describe('growing a landing forwards', () => {
  async function landed(newerPages: Message[][]): Promise<{
    thread: Thread;
    dirs: (string | undefined)[];
  }> {
    TestBed.configureTestingModule({
      providers: [
        provideZonelessChangeDetection(),
        provideRouter([]),
        { provide: MessagesApi, useValue: makeApi() },
        {
          provide: ActivatedRoute,
          useValue: {
            snapshot: { queryParamMap: convertToParamMap({ at: '5000_9' }) },
            queryParamMap: of(convertToParamMap({ at: '5000_9' })),
          },
        },
      ],
    });
    const fixture = TestBed.createComponent(Thread);
    const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
    const dirs: (string | undefined)[] = [];
    let n = 0;
    api.messages.mockImplementation(
      (_o: unknown, _i: unknown, _c?: string, _l?: number, dir?: string) => {
        dirs.push(dir);
        // THREE directions, and the landing's is `at` — inclusive of the hit.
        //
        // ⚠ The older half is exhausted ON PURPOSE. Every rect is zero in
        // jsdom, so `step()` reports the viewport as at BOTH edges at once and
        // `onScroll` would take the backward path as well, leaving these
        // assertions measuring `fetchOlder`. `has_more: false` retires it.
        if (dir === 'older') return page([msg('o1', 3000)], false, null);
        // `newerPages[0]` opens the landing (it contains the hit); the rest are
        // what scrolling forward fetches. ⚠ `prev_cursor` is what `fetchNewer`
        // continues from — left null, the helper's default, it returns at its
        // guard and the fetch never happens, which reads as the append being
        // wrong rather than absent.
        const batch = newerPages[n] ?? [];
        const more = n < newerPages.length - 1;
        n++;
        return page(batch, more, null, 'newer-c');
      },
    );
    fixture.componentRef.setInput('origin', 'irc');
    fixture.componentRef.setInput('id', '7');
    fixture.detectChanges();
    const thread = fixture.componentInstance;
    for (let i = 0; i < 40 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    // The message block is a viewChild, and `step()` returns "nothing needed"
    // when it has not been rendered — so without this the scroll below measures
    // an engine that declined to look.
    fixture.detectChanges();
    return { thread, dirs };
  }

  it('fetches forwards and appends, keeping order', async () => {
    const { thread } = await landed([[msg('h', 5000)], [msg('n1', 6000)]]);
    expect(thread.floating()).toBe(true);
    thread.fetchNewer();
    for (let i = 0; i < 20 && thread.loadingNewer(); i++) await new Promise((r) => setTimeout(r, 0));
    expect(thread.messages().map((m) => m.id)).toEqual(['o1', 'h', 'n1']);
  });

  /** ⚠ **Running out is how a landing rejoins the present**, and this is the
   *  test that says so. `floating` suppresses `pollNewer`; a reader who scrolled
   *  all the way forward would otherwise sit at the live end of the
   *  conversation and never see another message arrive — a stranger failure
   *  than the one #1401 is about, and one nothing on screen would explain. */
  it('rejoins the present once the forward pages run out', async () => {
    const { thread } = await landed([[msg('h', 5000)], [msg('n1', 6000)]]);
    thread.fetchNewer();
    for (let i = 0; i < 20 && thread.loadingNewer(); i++) await new Promise((r) => setTimeout(r, 0));
    expect(thread.floating()).toBe(false);
  });

  /** A window anchored to the present has nothing to fetch forwards, and asking
   *  would be a request per scroll that can only ever answer "nothing". */
  it('does not fetch forwards when it is not floating', async () => {
    const { thread, dirs } = await landed([[msg('h', 5000)]]);
    expect(thread.floating()).toBe(false);
    const before = dirs.filter((d) => d === 'newer').length;
    thread.fetchNewer();
    await new Promise((r) => setTimeout(r, 0));
    expect(dirs.filter((d) => d === 'newer').length).toBe(before);
  });
});

/** ⚠ **A `?at` THE SERVER CANNOT READ MUST NOT BUILD A NONSENSE THREAD.**
 *
 *  The backend treats a malformed cursor as absent, which is right for a
 *  backward page — it means "start at the newest". For a FORWARD page it means
 *  "everything after nothing", and the query answers with the OLDEST page. So
 *  the two halves of a landing come back from opposite ends of the archive and
 *  concatenate into a thread that jumps years mid-scroll, in order nowhere.
 *
 *  Reachable from a hand-edited URL or a bookmark predating the cursor format.
 *  The halves not joining is the signal, and a plain newest-page load is the
 *  answer — the same thing the app does with no `?at` at all. */
describe('landing with a cursor the server could not read', () => {
  it('falls back to the newest page rather than joining two ends', async () => {
    TestBed.configureTestingModule({
      providers: [
        provideZonelessChangeDetection(),
        provideRouter([]),
        { provide: MessagesApi, useValue: makeApi() },
        {
          provide: ActivatedRoute,
          useValue: {
            snapshot: { queryParamMap: convertToParamMap({ at: 'not-a-cursor' }) },
            queryParamMap: of(convertToParamMap({ at: 'not-a-cursor' })),
          },
        },
      ],
    });
    const fixture = TestBed.createComponent(Thread);
    const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
    api.messages.mockImplementation(
      (_o: unknown, _i: unknown, cursor?: string, _l?: number, dir?: string) => {
        // What the server really does with an unreadable cursor.
        if (dir === 'at') return page([msg('oldest', 1000)], true, null, 'c');
        if (dir === 'older') return page([msg('newest', 9_000_000)], true, 'c');
        return page([msg('newest', 9_000_000)], true, 'c');
      },
    );
    fixture.componentRef.setInput('origin', 'irc');
    fixture.componentRef.setInput('id', '7');
    fixture.detectChanges();
    const thread = fixture.componentInstance;
    for (let i = 0; i < 40 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));

    // Not ['newest', 'oldest'] — a thread that runs backwards.
    expect(thread.messages().map((m) => m.id)).toEqual(['newest']);
    // And not floating: this is the newest page, so the poll must keep running.
    expect(thread.floating()).toBe(false);
  });
});

/** **Landing on the right message is not the same as SHOWING which one.**
 *
 *  `scrollToTs` puts the hit flush under the sticky header and nothing marked
 *  it, so in a busy channel — `#netchat` at 1:07 PM has a dozen lines that look
 *  alike — you arrive in the right place and then have to work out which line
 *  you came for. Measured on the phone 2026-09-08 against a hit from 2013.
 *
 *  The marker's lifetime is the SAME as `?at`'s, deliberately: both mean "this
 *  is where you were put", and both stop being true the moment the reader
 *  scrolls. One rule, cleared in one place. */
describe('marking the message that was landed on', () => {
  async function land(): Promise<Thread> {
    TestBed.configureTestingModule({
      providers: [
        provideZonelessChangeDetection(),
        provideRouter([]),
        { provide: MessagesApi, useValue: makeApi() },
        {
          provide: ActivatedRoute,
          useValue: {
            snapshot: { queryParamMap: convertToParamMap({ at: '5000_9' }) },
            queryParamMap: of(convertToParamMap({ at: '5000_9' })),
          },
        },
      ],
    });
    const fixture = TestBed.createComponent(Thread);
    const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
    api.messages.mockImplementation(
      (_o: unknown, _i: unknown, _c?: string, _l?: number, dir?: string) =>
        dir === 'at'
          ? page([msg('h', 5000), msg('n1', 6000)], true, null, 'c')
          : page([msg('o1', 4000)], true, 'older-c'),
    );
    fixture.componentRef.setInput('origin', 'irc');
    fixture.componentRef.setInput('id', '7');
    fixture.detectChanges();
    const thread = fixture.componentInstance;
    for (let i = 0; i < 40 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    return thread;
  }

  it('names the hit, and not the message beside it', async () => {
    const thread = await land();
    expect(thread.landedId()).toBe('h');
  });

  it('stops naming it once the reader has moved', async () => {
    const thread = await land();
    expect(thread.landedId()).toBe('h');
    thread.commitFromParam();
    expect(thread.landedId()).toBeNull();
  });

  /** Opening a thread normally marks nothing — there is no message the reader
   *  was "put on", and a tint on the newest line would be noise on every open. */
  it('marks nothing when the thread was opened without a hit', async () => {
    const { thread } = await opened([msg('1', 100), msg('2', 200)]);
    expect(thread.landedId()).toBeNull();
  });
});
