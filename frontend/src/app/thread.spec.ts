import { ComponentRef, provideZonelessChangeDetection } from '@angular/core';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { ActivatedRoute, Router, convertToParamMap, provideRouter } from '@angular/router';
import { BehaviorSubject, Observable, of } from 'rxjs';
import { describe, expect, it, vi } from 'vitest';

import { Thread } from './thread';
import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
import { Conversation, Message, MessagesPage, ReplyTo } from './models';
import { MAX_RESTORE_PAGES } from './thread-window';

function msg(id: string, ts: number): Message {
  return { id, ts, sender: 's', is_outgoing: false, kind: 'message', body: 'b', deleted: false, edited: false, reactions: [], attachments: [], link_images: [], link_offers: [], edits: [], reply_to: null, delivery: null, entities: [], album: null, previews: [] };
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
  // createComponent, since the component injects ElementRef.
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

/** A thread routed to an IRC conversation and settled on `held`. The initial
 *  load must finish first: `pollNewer` declines to run during one. */
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
    thread.messages.set([msg('a', d1), msg('b', d1b), msg('c', d2)]);
    const groups = thread.dayGroups();
    expect(groups.length).toBe(2);
    expect(groups[0].runs.map((r) => r.map((m) => m.id))).toEqual([['a'], ['b']]);
    expect(groups[1].runs.map((r) => r.map((m) => m.id))).toEqual([['c']]);
  });

  it('dayGroups joins adjacent members of one album into a run', () => {
    const { thread } = setup();
    const at = (h: number): number => new Date(2026, 5, 1, h, 0, 0).getTime();
    const inAlbum = (id: string, h: number, album: string, sender = 's'): Message => ({
      ...msg(id, at(h)),
      album,
      sender,
    });
    thread.messages.set([
      msg('a', at(8)),
      inAlbum('p1', 9, 'A'),
      inAlbum('p2', 9, 'A'),
      inAlbum('p3', 9, 'A'),
      // Another album straight after: a new run, not a longer one.
      inAlbum('q1', 10, 'B'),
      // Same album id, different sender: not the same set.
      inAlbum('q2', 10, 'B', 'other'),
      msg('z', at(11)),
      // A member separated from its album by another message stands alone.
      inAlbum('p4', 12, 'A'),
    ]);
    expect(thread.dayGroups()[0].runs.map((r) => r.map((m) => m.id))).toEqual([
      ['a'],
      ['p1', 'p2', 'p3'],
      ['q1'],
      ['q2'],
      ['z'],
      ['p4'],
    ]);
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
    // Origin filter kept; `from` cleared.
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

    // irssi refuses, usually for want of an open tab.
    api.send.mockReturnValueOnce(of({ sent: false, error: 'refused: no conversation open with that target', archived: false }));
    thread.draft.set('please keep me');
    await thread.send();
    expect(thread.draft()).toBe('please keep me');
    expect(thread.sendError()).toContain('no conversation open');

    // The box empties only on a real send.
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

    // Sent, but the echo was not in irssi's log.
    api.send.mockReturnValueOnce(of({ sent: true, error: null, archived: false }));
    thread.draft.set('gone, but not seen');
    await thread.send();
    expect(thread.draft()).toBe('');
    expect(thread.sendError()).toContain('after the next import');
  });

  it('merges messages that arrived since the page was loaded', async () => {
    const { thread, api } = await opened([msg('1', 100), msg('2', 200)]);
    api.messages.mockReturnValueOnce(page([msg('1', 100), msg('2', 200), msg('3', 300)]));
    await thread.pollNewer();
    expect(thread.messages().map((m) => m.id)).toEqual(['1', '2', '3']);
  });

  it('puts a late-arriving older line in its place rather than at the end', async () => {
    const { thread, api } = await opened([msg('1', 100), msg('3', 300)]);
    // An import can land an older line late.
    api.messages.mockReturnValueOnce(page([msg('1', 100), msg('2', 200), msg('3', 300)]));
    await thread.pollNewer();
    expect(thread.messages().map((m) => m.id)).toEqual(['1', '2', '3']);
  });

  it('reloads instead of merging when the newest page overlaps nothing held', async () => {
    const { thread, api } = await opened([msg('1', 100)]);
    // None of the page is known, so there is a gap: reload rather than merge.
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
// Formatting is covered by copy-log.spec.ts; these check that a real Range picks
// the right messages and the two-message threshold holds.

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

/** A reply to something rendered scrolls without navigating; one outside the
 *  rendered window lands via `?at`, since `scrollToTs` would pick a neighbour. */
describe('Thread reply jump', () => {
  const replyTo = (over: Partial<ReplyTo> = {}): ReplyTo => ({
    id: '1',
    cursor: '100_1',
    ts: 100,
    sender: 's',
    excerpt: 'b',
    deleted: false,
    ...over,
  });

  it('scrolls to a message already on screen, without navigating', async () => {
    const { thread } = await opened([msg('1', 100), msg('2', 200)]);
    const router = TestBed.inject(Router);
    const nav = vi.spyOn(router, 'navigate').mockResolvedValue(true);

    thread.jumpToReply(replyTo());

    expect(nav).not.toHaveBeenCalled();
    // Marked, as a search landing is.
    expect(thread.landedId()).toBe('1');
  });

  it('navigates by cursor when the message is not in the rendered window', async () => {
    const { thread } = await opened([msg('5', 500)]);
    const router = TestBed.inject(Router);
    const nav = vi.spyOn(router, 'navigate').mockResolvedValue(true);

    thread.jumpToReply(replyTo({ id: '1', cursor: '100_1' }));

    expect(nav).toHaveBeenCalledWith(
      [],
      expect.objectContaining({
        // `from` goes with it.
        queryParams: { at: '100_1', from: null },
        queryParamsHandling: 'merge',
      }),
    );
    expect(thread.landedId()).toBeNull();
  });

  it('does nothing for a reply the archive cannot resolve', async () => {
    const { thread } = await opened([msg('5', 500)]);
    const router = TestBed.inject(Router);
    const nav = vi.spyOn(router, 'navigate').mockResolvedValue(true);

    thread.jumpToReply(replyTo({ id: null, cursor: null, excerpt: null }));

    expect(nav).not.toHaveBeenCalled();
    expect(thread.landedId()).toBeNull();
  });
});

describe('Thread rendering', () => {
  it('draws an action with its star, which the backend no longer sends', async () => {
    // The body is the words alone; the template supplies the star.
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

describe('Thread edit history', () => {
  async function withEdited(): Promise<ComponentFixture<Thread>> {
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
    api.messages.mockReturnValue(
      page([
        {
          ...msg('e', new Date(2026, 7, 13, 14, 30).getTime()),
          body: 'what it says now',
          edited: true,
          edits: [
            { ts: new Date(2026, 7, 13, 14, 30).getTime(), body: 'first thought' },
            { ts: new Date(2026, 7, 13, 14, 32).getTime(), body: 'second thought' },
          ],
        },
      ]),
    );
    ref.setInput('origin', 'signal');
    ref.setInput('id', 'dm:a');
    fixture.detectChanges();
    for (let i = 0; i < 20 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    fixture.detectChanges();
    return fixture;
  }

  it('shows only what the message says now', async () => {
    // One bubble for the versions, not two.
    const el = (await withEdited()).nativeElement as HTMLElement;
    expect(el.querySelector('.msg .body')?.textContent).toContain('what it says now');
    expect(el.textContent).not.toContain('first thought');
    expect(el.textContent).not.toContain('second thought');
  });

  it('opens what it said before, oldest first', async () => {
    const f = await withEdited();
    const el = f.nativeElement as HTMLElement;
    el.querySelector<HTMLButtonElement>('.tag.history-toggle')!.click();
    f.detectChanges();
    const said = [...el.querySelectorAll('.edit-history .said')].map((e) => e.textContent?.trim());
    expect(said).toEqual(['first thought', 'second thought']);
    // The current text is last in the bubble.
    expect(el.querySelector('.msg .body')?.textContent).toContain('what it says now');
  });

  it('closes again, so a history is something you ask for', async () => {
    const f = await withEdited();
    const el = f.nativeElement as HTMLElement;
    el.querySelector<HTMLButtonElement>('.tag.history-toggle')!.click();
    f.detectChanges();
    el.querySelector<HTMLButtonElement>('.tag.history-toggle')!.click();
    f.detectChanges();
    expect(el.querySelector('.edit-history')).toBeNull();
  });

  it('an edited message with no stored history is still marked, without a control', async () => {
    // No versions: the tag must not become a button that opens nothing.
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
    api.messages.mockReturnValue(page([{ ...msg('e', 100), edited: true, edits: [] }]));
    ref.setInput('origin', 'gchat');
    ref.setInput('id', 'gc1');
    fixture.detectChanges();
    for (let i = 0; i < 20 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    expect(el.querySelector('.tag')?.textContent).toContain('edited');
    expect(el.querySelector('.tag.history-toggle')).toBeNull();
  });
});

describe('Thread link offers', () => {
  async function withOffer(): Promise<{ f: ComponentFixture<Thread>; api: { requestLinkImage: ReturnType<typeof vi.fn>; linkImageState: ReturnType<typeof vi.fn> } }> {
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as {
      messages: ReturnType<typeof vi.fn>;
      requestLinkImage: ReturnType<typeof vi.fn>;
      linkImageState: ReturnType<typeof vi.fn>;
    };
    api.messages.mockReturnValue(
      page([
        {
          ...msg('d', 100),
          body: 'look https://cloud.example.org/nc/s/T',
          link_offers: [{ url: 'https://cloud.example.org/nc/s/T', id: 'h1' }],
        },
      ]),
    );
    api.requestLinkImage = vi.fn().mockReturnValue(of({ state: 'ok', content_type: 'image/jpeg' }));
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();
    for (let i = 0; i < 20 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    fixture.detectChanges();
    return { f: fixture, api };
  }

  it('offers a control and fetches NOTHING until it is tapped', async () => {
    // Opening a conversation only offers links; it contacts no other server.
    const { f, api } = await withOffer();
    const el = f.nativeElement as HTMLElement;
    expect(el.querySelector('.link-offer button')?.textContent).toContain('Show picture');
    expect(el.querySelectorAll('.msg img').length).toBe(0);
    expect(api.requestLinkImage).not.toHaveBeenCalled();
  });

  it('asks by the id the page offered, never by a URL', async () => {
    const { f, api } = await withOffer();
    (f.nativeElement as HTMLElement).querySelector<HTMLButtonElement>('.link-offer button')!.click();
    f.detectChanges();
    expect(api.requestLinkImage).toHaveBeenCalledWith('h1');
  });

  it('shows the picture the request answered with, in place', async () => {
    // The answer arrives on the request.
    const { f } = await withOffer();
    (f.nativeElement as HTMLElement).querySelector<HTMLButtonElement>('.link-offer button')!.click();
    f.detectChanges();
    const el = f.nativeElement as HTMLElement;
    expect(el.querySelector('.link-offer')).toBeNull();
    expect(el.querySelector('.msg img')?.getAttribute('src')).toBe('/api/link-images/h1');
  });

  it('takes the control away when the link turns out not to be a picture', async () => {
    const { f, api } = await withOffer();
    api.requestLinkImage.mockReturnValue(of({ state: 'not_image', content_type: null }));
    (f.nativeElement as HTMLElement).querySelector<HTMLButtonElement>('.link-offer button')!.click();
    f.detectChanges();
    const el = f.nativeElement as HTMLElement;
    expect(el.querySelector('.link-offer')).toBeNull();
    expect(el.querySelectorAll('.msg img').length).toBe(0);
  });
});

describe('Thread linked pictures', () => {
  it('serves a linked picture from us, never from the other server', async () => {
    // Served by us, never from the original link.
    const f = await withLinkImage();
    const img = (f.nativeElement as HTMLElement).querySelector('.msg[data-id="d"] img')!;
    expect(img.getAttribute('src')).toBe('/api/link-images/abc123');
  });

  it('still links out to where the picture came from', async () => {
    const f = await withLinkImage();
    const a = (f.nativeElement as HTMLElement).querySelector('.msg[data-id="d"] .link-image a')!;
    expect(a.getAttribute('href')).toBe('https://cloud.example.org/nc/s/TOKEN');
    expect(a.getAttribute('rel')).toContain('noreferrer');
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
    // Half a bubble selected is that message selected.
    const fixture = await threeRendered();
    select(fixture, ['a', 6], ['b', 1]);
    expect(fireCopy(fixture).written.get('text/plain')).toBe(
      '--- Day changed Thu Aug 13 2026\n14:32 <pippijn> hello there\n14:33 <simon> hi',
    );
  });

  it('leaves a selection inside one message to the browser', async () => {
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

  /** Screen and clipboard name an attachment the same way (`attachment.ts`). */
  it('calls a nameless unavailable image what it is, as the clipboard does', async () => {
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
    api.messages.mockReturnValue(
      page([
        {
          ...msg('a', new Date(2026, 7, 13, 14, 32).getTime()),
          attachments: [
            { id: 'x', content_type: 'image/jpeg', file_name: null, size: null, available: false, is_image: true, fetch: null },
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

/** A message whose text links a picture we hold; `deleted` puts it behind the
 *  reveal. */
async function withLinkImage(deleted = false): Promise<ComponentFixture<Thread>> {
  const { thread, ref, fixture } = setup();
  const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
  const m: Message = {
    ...msg('d', new Date(2026, 7, 13, 14, 33).getTime()),
    body: deleted ? null : 'look at https://cloud.example.org/nc/s/TOKEN',
    deleted,
    link_images: [
      { url: 'https://cloud.example.org/nc/s/TOKEN', id: 'abc123', content_type: 'image/jpeg' },
    ],
    link_offers: [],
  };
  api.messages.mockReturnValue(page([m]));
  ref.setInput('origin', 'irc');
  ref.setInput('id', '7');
  fixture.detectChanges();
  for (let i = 0; i < 20 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
  fixture.detectChanges();
  return fixture;
}

/** One ordinary message and one deleted one, rendered. `withImage` gives the
 *  deleted one an image instead of a body. */
async function withDeleted(withImage = false): Promise<ComponentFixture<Thread>> {
  const { thread, ref, fixture } = setup();
  const api = TestBed.inject(MessagesApi) as unknown as { messages: ReturnType<typeof vi.fn> };
  const del: Message = withImage
    ? {
        ...msg('d', new Date(2026, 7, 13, 14, 33).getTime()),
        body: null,
        deleted: true,
        attachments: [
          { id: 'i1', content_type: 'image/jpeg', file_name: 'x.jpg', size: 10, available: true, is_image: true, fetch: null },
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

  /** A deleted message's pictures are hidden too. With no body there is no
   *  `.body` div, so this keys on the bubble. */
  it('does not render a deleted message\'s images until it is revealed', async () => {
    const f = await withDeleted(true);
    const bubble = (f.nativeElement as HTMLElement).querySelector('.msg[data-id="d"]')!;
    expect(bubble.querySelectorAll('img').length).toBe(0);

    revealBtn(f)!.click();
    f.detectChanges();
    expect(bubble.querySelectorAll('img').length).toBe(1);
  });

  /** Likewise a picture fetched for a link. */
  it('does not render a deleted message\'s LINKED pictures until it is revealed', async () => {
    const f = await withLinkImage(true);
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

  /** Revealing is for this screen only; the clipboard keeps `(deleted)`. */
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

  /** A select-all copies only the rendered window, and says so. */
  it('a whole-window copy in a truncated thread says what was left out', async () => {
    const f = await opened3(401794);
    select(f, ['a', 0], ['c', 7]);
    expect(fireCopy(f).written.get('text/plain')).toContain('copied 3 of 401794 messages');
  });

  /** An ordinary selection gets no such note. */
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

  /** While an IME is composing, Enter accepts the candidate rather than sending. */
  it('leaves a composing Enter to the IME rather than answering it', async () => {
    const { thread, api } = await composer();
    thread.draft.set('hello wor');
    const e = enter({ isComposing: true });
    thread.onComposerKey(e);
    expect(api.send).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
  });

  /** `send` itself refuses mid-composition: an Enter the handler ignores still
   *  submits the form. The real composition is driven in e2e/ui-pages.spec.ts. */
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
/** A stale `?from` older than anything returned, with the API always claiming
 *  another page. */
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
    // Every page is newer than `from`, so only the bound ends the loop. The
    // supply is finite so that removing the bound fails rather than hangs.
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

  it('stops after a bounded number of requests when `from` is unreachable', async () => {
    const { calls } = await openWithFrom('1');
    expect(calls).toBeLessThanOrEqual(MAX_RESTORE_PAGES + 1);
    // It did page back, not stop at once.
    expect(calls).toBeGreaterThan(1);
  });

  it('stops as soon as the saved depth is reached, well inside the bound', async () => {
    const { calls } = await openWithFrom('5000');
    expect(calls).toBe(1);
  });
});

/** Landing on a search hit. `?at` means "put me here", `?from` "I was here";
 *  `at` wins on load and `commitFromParam` replaces it on the first scroll. */
describe('landing on a search hit', () => {
/** `newerHasMore`: whether the conversation runs on past the hit. */
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
        // The two before the hit, and the hit with the two after it. The
        // forward half is `at`, inclusive of the hit.
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
    expect(calls.every((c) => c.cursor === '5000_9')).toBe(true);
    expect(calls.map((c) => c.dir).sort()).toEqual(['at', 'older']);
  });

  it('holds the hit with context after it, not only before it', async () => {
    const { thread } = await openAt('5000_9');
    const ids = thread.messages().map((m) => m.id);
    expect(ids).toEqual(['o1', 'o2', 'h', 'n1', 'n2']);
    // Messages after the hit are loaded too.
    expect(ids.indexOf('h')).toBeLessThan(ids.length - 1);
  });

  it('is not treated as being at the latest message', async () => {
    const { thread } = await openAt('5000_9');
    // Floating, so `pollNewer` does not reload into the present.
    expect(thread.floating()).toBe(true);
  });

  /** A landing at the end is not floating, so polling continues. */
  it('a hit near the end of the conversation is at the latest message', async () => {
    const { thread } = await openAt('5000_9', false);
    expect(thread.floating()).toBe(false);
  });
});

/** A hit in the conversation already open changes only the query string, which
 *  must still reload. */
describe('landing again without changing conversation', () => {
  it('re-lands when only ?at changes', async () => {
    const params: Record<string, string> = { at: '5000_9' };
    // The real `queryParamMap` stream, so the subscription is tested.
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

    qp.next(convertToParamMap({ at: '9000_11' }));
    for (let i = 0; i < 40 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    expect(loads).toBe(2);
  });
});

/** Scrolling forward off a landing. */
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
        // The landing's direction is `at`. The older half is exhausted: every
        // rect is zero in jsdom, so `step()` sees both edges and would fetch
        // older too.
        if (dir === 'older') return page([msg('o1', 3000)], false, null);
        // `newerPages[0]` opens the landing; the rest are fetched by scrolling.
        // `prev_cursor` must be set, or `fetchNewer` returns at its guard.
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
    // Render the message block, or `step()` has nothing to measure.
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

  /** The forward page running out ends `floating`, so polling resumes. */
  it('rejoins the present once the forward pages run out', async () => {
    const { thread } = await landed([[msg('h', 5000)], [msg('n1', 6000)]]);
    thread.fetchNewer();
    for (let i = 0; i < 20 && thread.loadingNewer(); i++) await new Promise((r) => setTimeout(r, 0));
    expect(thread.floating()).toBe(false);
  });

  /** A window at the present fetches nothing forwards. */
  it('does not fetch forwards when it is not floating', async () => {
    const { thread, dirs } = await landed([[msg('h', 5000)]]);
    expect(thread.floating()).toBe(false);
    const before = dirs.filter((d) => d === 'newer').length;
    thread.fetchNewer();
    await new Promise((r) => setTimeout(r, 0));
    expect(dirs.filter((d) => d === 'newer').length).toBe(before);
  });
});

/** An unreadable `?at` makes the halves the archive's two ends (a malformed
 *  cursor reads as absent), so the landing falls back to the newest page. */
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

    expect(thread.messages().map((m) => m.id)).toEqual(['newest']);
    // Not floating: this is the newest page.
    expect(thread.floating()).toBe(false);
  });
});

/** The landed message is marked, and the mark clears with `?at` on the first
 *  scroll. */
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

  /** An ordinary open marks nothing. */
  it('marks nothing when the thread was opened without a hit', async () => {
    const { thread } = await opened([msg('1', 100), msg('2', 200)]);
    expect(thread.landedId()).toBeNull();
  });
});

/** The delivery tag claims only what is known: in a group, a count of readers
 *  rather than "read". */
describe('the delivery tag', () => {
  function outgoing(id: string, delivery: Message['delivery']): Message {
    return { ...msg(id, 1000), is_outgoing: true, delivery };
  }

  async function render(messages: Message[], conversations: Conversation[] = []) {
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as {
      messages: ReturnType<typeof vi.fn>;
      conversations: ReturnType<typeof vi.fn>;
    };
    api.conversations.mockReturnValue(of(conversations));
    api.messages.mockReturnValue(page(messages));
    TestBed.inject(MessagesStore).refresh();
    ref.setInput('origin', 'signal');
    ref.setInput('id', 'dm:a');
    fixture.detectChanges();
    for (let i = 0; i < 20 && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
    fixture.detectChanges();
    const root = fixture.nativeElement as HTMLElement;
    return (id: string) => root.querySelector(`.msg[data-id="${id}"] .tag.delivery`);
  }

  const dm: Conversation = {
    origin: 'signal', id: 'dm:a', name: 'Alice', kind: 'dm',
    network: null, message_count: 1, last_ts: 1000,
  };

  it('says nothing at all when the archive cannot tell', async () => {
    // Sent before capture began: no tag.
    const tag = await render([outgoing('a', null)]);
    expect(tag('a')).toBeNull();
  });

  it('shows each rung of the ladder, dimmed until somebody has read it', async () => {
    const tag = await render([
      outgoing('s', { state: 'sent', read_by: [] }),
      outgoing('d', { state: 'delivered', read_by: [] }),
      outgoing('r', { state: 'read', read_by: [] }),
    ]);
    expect(tag('s')?.textContent?.trim()).toBe('sent');
    expect(tag('d')?.textContent?.trim()).toBe('delivered');
    expect(tag('r')?.textContent?.trim()).toBe('read');
    expect(tag('s')?.classList.contains('unread')).toBe(true);
    expect(tag('d')?.classList.contains('unread')).toBe(true);
    expect(tag('r')?.classList.contains('unread')).toBe(false);
  });

  it('names the reader and the time in the title, where the origin records it', async () => {
    const at = new Date(2026, 8, 20, 14, 5).getTime();
    const tag = await render([outgoing('r', { state: 'read', read_by: [{ who: 'Alice', at }] })], [dm]);
    const title = tag('r')?.getAttribute('title') ?? '';
    expect(title).toContain('Alice');
    expect(title).toMatch(/\d/);
  });

  it('carries an EMPTY title when the origin names nobody', async () => {
    // Telegram names nobody: an empty string, not "undefined".
    const bare = await render([outgoing('t', { state: 'read', read_by: [] })], [dm]);
    expect(bare('t')?.getAttribute('title')).toBe('');
  });

  it('counts readers in a group and never says plain "read"', async () => {
    const group: Conversation = { ...dm, kind: 'group', name: 'Grp' };
    const two = [
      { who: 'Alice', at: 1100 },
      { who: 'Bob', at: 1200 },
    ];
    const tag = await render([outgoing('g', { state: 'read', read_by: two })], [group]);
    expect(tag('g')?.textContent?.trim()).toBe('read by 2');
  });

  it('counts rather than asserts when the conversation is not loaded yet', async () => {
    // The kind is unknown before the list loads: the counted form.
    const tag = await render([outgoing('g', { state: 'read', read_by: [{ who: 'Alice', at: 1100 }] })]);
    expect(tag('g')?.textContent?.trim()).toBe('read by 1');
  });

  it('says plain "read" in a DM, where one reader IS everybody', async () => {
    const tag = await render([outgoing('r', { state: 'read', read_by: [{ who: 'Alice', at: 1100 }] })], [dm]);
    expect(tag('r')?.textContent?.trim()).toBe('read');
  });
});

/** Entity offsets are UTF-16 code units, as `String.prototype.slice` counts. */
describe('formatted message bodies', () => {
  function withEntities(body: string, entities: Message['entities']): Message {
    return { ...msg('m', 1000), body, entities };
  }
  /** One TestBed per test; the splitter is pure, so one instance answers all. */
  interface Seg { text: string; cls: string; href: string | null }
  const segsWith = (t: Thread, m: Message): Seg[] =>
    (t as unknown as { segments(m: Message): Seg[] }).segments(m);

  it('splits a body into plain and formatted runs', () => {
    const { thread } = setup();
    const m = withEntities('hello brave world', [
      { kind: 'bold', offset: 6, length: 5, url: null },
    ]);
    expect(segsWith(thread, m)).toEqual([
      { text: 'hello ', cls: '', href: null },
      { text: 'brave', cls: 'fmt-bold', href: null },
      { text: ' world', cls: '', href: null },
    ]);
  });

  it('counts an emoji as TWO, because Telegram and Signal do', () => {
    // '👋' is two code units, so offset 2 is after it.
    const { thread } = setup();
    const m = withEntities('👋 bold', [{ kind: 'bold', offset: 3, length: 4, url: null }]);
    expect(segsWith(thread, m).map((s) => s.text)).toEqual(['👋 ', 'bold']);
  });

  it('never lets an entity change how much of the message is shown', () => {
    const { thread } = setup();
    const body = 'short';
    // Past the end: clamped.
    const over = withEntities(body, [{ kind: 'bold', offset: 2, length: 99, url: null }]);
    expect(segsWith(thread, over).map((s) => s.text).join('')).toBe(body);
  });

  it('combines overlapping runs rather than dropping one', () => {
    const { thread } = setup();
    // Signal's own shape: one span both monospace and struck, and a nested run.
    const m = withEntities('abcdefgh mono', [
      { kind: 'bold', offset: 0, length: 5, url: null },
      { kind: 'italic', offset: 2, length: 3, url: null },
      { kind: 'code', offset: 9, length: 4, url: null },
      { kind: 'strike', offset: 9, length: 4, url: null },
    ]);
    expect(segsWith(thread, m)).toEqual([
      { text: 'ab', cls: 'fmt-bold', href: null },
      { text: 'cde', cls: 'fmt-bold fmt-italic', href: null },
      { text: 'fgh ', cls: '', href: null },
      { text: 'mono', cls: 'fmt-code fmt-strike', href: null },
    ]);
  });

  it('keeps a link whole when formatting splits it', () => {
    const { thread } = setup();
    const m = withEntities('see https://x.org now', [
      { kind: 'url', offset: 4, length: 13, url: null },
      { kind: 'bold', offset: 12, length: 5, url: null },
    ]);
    expect(segsWith(thread, m)).toEqual([
      { text: 'see ', cls: '', href: null },
      { text: 'https://', cls: 'fmt-url', href: 'https://x.org' },
      { text: 'x.org', cls: 'fmt-url fmt-bold', href: 'https://x.org' },
      { text: ' now', cls: '', href: null },
    ]);
  });

  it('is exactly the body when nothing is formatted', () => {
    const { thread } = setup();
    const m = withEntities('just words', []);
    expect(segsWith(thread, m)).toEqual([{ text: 'just words', cls: '', href: null }]);
  });
});
