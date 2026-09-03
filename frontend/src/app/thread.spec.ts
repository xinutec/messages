import { ComponentRef, provideZonelessChangeDetection } from '@angular/core';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { Router, provideRouter } from '@angular/router';
import { Observable, of } from 'rxjs';
import { describe, expect, it, vi } from 'vitest';

import { Thread } from './thread';
import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
import { Message, MessagesPage } from './models';

function msg(id: string, ts: number): Message {
  return { id, ts, sender: 's', is_outgoing: false, kind: 'message', body: 'b', deleted: false, edited: false, reactions: [], attachments: [] };
}

function makeApi() {
  return {
    me: vi.fn(() => of({ user_id: 'u1', display_name: 'Test User' })),
    conversations: vi.fn(() => of([])),
    messages: vi.fn(() => of({ messages: [msg('1', 100)], has_more: false, next_cursor: null } as MessagesPage)),
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

function page(messages: Message[], has_more = false, next_cursor: string | null = null): Observable<MessagesPage> {
  return of({ messages, has_more, next_cursor });
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