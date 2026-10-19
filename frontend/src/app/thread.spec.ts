import { Location } from '@angular/common';
import { ComponentRef, provideZonelessChangeDetection } from '@angular/core';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { ActivatedRoute, Router, convertToParamMap, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { BehaviorSubject, Observable, Subject, of } from 'rxjs';
import { describe, expect, it, vi } from 'vitest';
import { Place, declaredUp, resolveUp } from '@xinutec/ui-scaffold';

import { routes } from './app.routes';
import { Thread } from './thread';
import { MessagesApi } from './messages-api';
import { type FakeApi, fakeApi, fakeApiOf } from './test-api';
import { MessagesStore } from './messages-store';
import { Conversation, Message, MessagesPage, ReplyTo } from './models';
import { testMessage } from './test-message';
import { MAX_RESTORE_PAGES } from './thread-window';

const msg = (id: string, ts: number): Message => testMessage({ id, ts });

function setup(): { thread: Thread; ref: ComponentRef<Thread>; fixture: ComponentFixture<Thread>; router: Router } {
  TestBed.configureTestingModule({
    providers: [
      provideZonelessChangeDetection(),
      provideRouter([]),
      { provide: MessagesApi, useValue: fakeApi() },
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

/** Let the thread finish loading: its fetches resolve on later ticks. */
async function settle(thread: Thread, tries = 40): Promise<void> {
  for (let i = 0; i < tries && thread.loadingThread(); i++) await new Promise((r) => setTimeout(r, 0));
}

/** A thread whose route carries `params`, as a deep link or a landing does. */
function setupWithQuery(params: Record<string, string>): ComponentFixture<Thread> {
  TestBed.configureTestingModule({
    providers: [
      provideZonelessChangeDetection(),
      provideRouter([]),
      { provide: MessagesApi, useValue: fakeApi() },
      {
        provide: ActivatedRoute,
        useValue: {
          snapshot: { queryParamMap: convertToParamMap(params) },
          queryParamMap: of(convertToParamMap(params)),
        },
      },
    ],
  });
  return TestBed.createComponent(Thread);
}

/** A thread routed to an IRC conversation and settled on `held`. The initial
 *  load must finish first: `pollNewer` declines to run during one. */
async function opened(held: Message[]): Promise<{ thread: Thread; api: { messages: ReturnType<typeof vi.fn> } }> {
  const { thread, ref, fixture } = setup();
  const api = fakeApiOf();
  api.messages.mockReturnValue(page(held));
  ref.setInput('origin', 'irc');
  ref.setInput('id', '7');
  fixture.detectChanges();
  await settle(thread);
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

  it('up from a conversation is the list, keeping the origin filter and dropping the paged depth', () => {
    const route = routes.find((r) => r.path === 'conversation/:origin/:id');
    const declared = declaredUp(route?.data ?? {});
    if (declared === undefined) throw new Error('the conversation route declares no up');
    const up = resolveUp(declared, { origin: 'irc', id: '#c' }, { origin: 'irc', from: '42' });
    expect(up).toEqual(expect.objectContaining({ path: '/', query: { origin: 'irc' } }));
  });
  it('offers a composer for IRC and a Telegram DM or group — never Signal, nor a broadcast', () => {
    const { thread, ref, fixture } = setup();
    const tg = (id: string, kind: Conversation['kind']): Conversation => ({
      origin: 'telegram', id, name: id, kind, network: null, message_count: 1, last_ts: 1, last: null, unread: 0, avatar: null,
    });
    TestBed.inject(MessagesStore).conversations.set([tg('4242', 'dm'), tg('-77', 'group'), tg('-1055', 'channel')]);
    const opens = (origin: string, id: string): boolean => {
      ref.setInput('origin', origin);
      ref.setInput('id', id);
      fixture.detectChanges();
      return thread.canSend();
    };
    expect(opens('signal', 'dm:a')).toBe(false);
    expect(opens('irc', '7')).toBe(true);
    expect(opens('telegram', '4242')).toBe(true);
    expect(opens('telegram', '-77')).toBe(true);
    expect(opens('telegram', '-1055')).toBe(false);
    // Not in the list yet: no composer until its kind is known.
    expect(opens('telegram', '999')).toBe(false);
  });

  it('keeps the draft when the send is refused, and clears it only once it went', async () => {
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();

    // irssi refuses, usually for want of an open tab.
    api.send.mockReturnValueOnce(of({ sent: false, error: 'refused: no conversation open with that target', archived: false }));
    thread.composer.draft.set('please keep me');
    await thread.composer.send();
    expect(thread.composer.draft()).toBe('please keep me');
    expect(thread.composer.error()).toContain('no conversation open');

    // The box empties only on a real send.
    api.send.mockReturnValueOnce(of({ sent: true, error: null, archived: true }));
    await thread.composer.send();
    expect(thread.composer.draft()).toBe('');
  });

  it('says so when a message went but is not in the archive yet', async () => {
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();

    // Sent, but the echo was not in irssi's log.
    api.send.mockReturnValueOnce(of({ sent: true, error: null, archived: false }));
    thread.composer.draft.set('gone, but not seen');
    await thread.composer.send();
    expect(thread.composer.draft()).toBe('');
    expect(thread.composer.error()).toContain('after the next import');

    // Telegram has no import to wait for.
    ref.setInput('origin', 'telegram');
    ref.setInput('id', '4242');
    fixture.detectChanges();
    api.send.mockReturnValueOnce(of({ sent: true, error: null, archived: false }));
    thread.composer.draft.set('gone, but not stored');
    await thread.composer.send();
    expect(thread.composer.error()).toBe('Sent, but not archived yet.');
  });

  it('drops an older page that arrives after the reader switched conversations', async () => {
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
    api.messages.mockReturnValue(page([msg('a2', 200)], true, 'older-than-a2'));
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();
    await settle(thread);

    // Scrolling up asks for conversation 7's older page, which is slow.
    const late = new Subject<MessagesPage>();
    api.messages.mockReturnValueOnce(late);
    thread.fetchOlder();

    // The reader opens conversation 8 before it arrives.
    api.messages.mockReturnValue(page([msg('b1', 500)]));
    ref.setInput('id', '8');
    fixture.detectChanges();
    await settle(thread);

    late.next({ messages: [msg('a1', 100)], has_more: false, next_cursor: null, prev_cursor: null });
    late.complete();
    expect(thread.messages().map((m) => m.id)).toEqual(['b1']);
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
    const api = fakeApiOf();
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();

    thread.composer.draft.set('   ');
    await thread.composer.send();
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
  const api = fakeApiOf();
  api.messages.mockReturnValue(
    page([
      { ...msg('a', new Date(2026, 7, 13, 14, 32).getTime()), sender: 'user', body: 'hello there' },
      { ...msg('b', new Date(2026, 7, 13, 14, 33).getTime()), sender: 'simon', body: 'hi' },
      { ...msg('c', new Date(2026, 7, 14, 9, 5).getTime()), sender: 'simon', body: 'morning' },
    ]),
  );
  ref.setInput('origin', 'irc');
  ref.setInput('id', '7');
  fixture.detectChanges();
  await settle(thread);
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

  /** The Thread routed at `url` by the app's own routes, settled on `held`. */
  async function routedAt(url: string, held: Message[]): Promise<{ thread: Thread; router: Router }> {
    const api = fakeApi();
    api.messages.mockReturnValue(page(held));
    TestBed.configureTestingModule({
      providers: [
        provideZonelessChangeDetection(),
        provideRouter(routes, withComponentInputBinding()),
        { provide: MessagesApi, useValue: api },
      ],
    });
    const harness = await RouterTestingHarness.create();
    const thread = await harness.navigateByUrl(url, Thread);
    await settle(thread);
    return { thread, router: TestBed.inject(Router) };
  }

  /** The URL once anything `act` started has had its turn. */
  async function urlAfter(router: Router, act: () => void): Promise<string> {
    act();
    await new Promise((r) => setTimeout(r, 0));
    return router.url;
  }

  it('scrolls to a message already on screen, without navigating', async () => {
    const { thread, router } = await routedAt('/conversation/irc/7?origin=irc', [msg('1', 100), msg('2', 200)]);

    expect(await urlAfter(router, () => thread.jumpToReply(replyTo()))).toBe('/conversation/irc/7?origin=irc');
    // Marked, as a search landing is.
    expect(thread.landedId()).toBe('1');
  });

  it('lands by cursor when the message is not in the rendered window', async () => {
    const { thread, router } = await routedAt('/conversation/irc/7?origin=irc', [msg('5', 500)]);
    const navigate = vi.spyOn(router, 'navigate');

    expect(await urlAfter(router, () => thread.jumpToReply(replyTo({ id: '1', cursor: '100_1' })))).toBe(
      '/conversation/irc/7?origin=irc&at=100_1',
    );
    // In place of where the reader was, as every jump within a conversation is:
    // back leaves the conversation.
    expect(navigate).toHaveBeenCalledWith([], expect.objectContaining({ replaceUrl: true }));
  });

  it('does nothing for a reply the archive cannot resolve', async () => {
    const { thread, router } = await routedAt('/conversation/irc/7?origin=irc', [msg('5', 500)]);

    expect(await urlAfter(router, () => thread.jumpToReply(replyTo({ id: null, cursor: null, excerpt: null })))).toBe(
      '/conversation/irc/7?origin=irc',
    );
    expect(thread.landedId()).toBeNull();
  });
});

describe('Thread rendering', () => {
  it('draws an action with its star, which the template supplies', async () => {
    // The body is the words alone; the template supplies the star.
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
    api.messages.mockReturnValue(
      page([
        { ...msg('m', 100), sender: 'alice', body: 'hello' },
        { ...msg('a', 200), sender: 'alice', kind: 'action', body: 'waves' },
      ]),
    );
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();
    await settle(thread);
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
    const api = fakeApiOf();
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
    await settle(thread);
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
    const api = fakeApiOf();
    api.messages.mockReturnValue(page([{ ...msg('e', 100), edited: true, edits: [] }]));
    ref.setInput('origin', 'gchat');
    ref.setInput('id', 'gc1');
    fixture.detectChanges();
    await settle(thread);
    fixture.detectChanges();
    const el = fixture.nativeElement as HTMLElement;
    expect(el.querySelector('.tag')?.textContent).toContain('edited');
    expect(el.querySelector('.tag.history-toggle')).toBeNull();
  });
});

describe('Thread link offers', () => {
  async function withOffer(): Promise<{ f: ComponentFixture<Thread>; api: FakeApi }> {
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
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
    await settle(thread);
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
        '14:32 <user> hello there\n' +
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
    expect(html).toContain('&lt;user&gt;');
  });

  it('takes a message the selection only clips, whole', async () => {
    // Half a bubble selected is that message selected.
    const fixture = await threeRendered();
    select(fixture, ['a', 6], ['b', 1]);
    expect(fireCopy(fixture).written.get('text/plain')).toBe(
      '--- Day changed Thu Aug 13 2026\n14:32 <user> hello there\n14:33 <simon> hi',
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
    const api = fakeApiOf();
    api.messages.mockReturnValue(
      page([
        {
          ...msg('a', new Date(2026, 7, 13, 14, 32).getTime()),
          attachments: [
            { id: 'x', content_type: 'image/jpeg', file_name: null, size: null, available: false, fetch: null, transcript: null },
          ],
        },
      ]),
    );
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();
    await settle(thread);
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
  const api = fakeApiOf();
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
  await settle(thread);
  fixture.detectChanges();
  return fixture;
}

/** One ordinary message and one deleted one, rendered. `withImage` gives the
 *  deleted one an image instead of a body. */
async function withDeleted(withImage = false): Promise<ComponentFixture<Thread>> {
  const { thread, ref, fixture } = setup();
  const api = fakeApiOf();
  const del: Message = withImage
    ? {
        ...msg('d', new Date(2026, 7, 13, 14, 33).getTime()),
        body: null,
        deleted: true,
        attachments: [
          { id: 'i1', content_type: 'image/jpeg', file_name: 'x.jpg', size: 10, available: true, fetch: null, transcript: null },
        ],
      }
    : { ...msg('d', new Date(2026, 7, 13, 14, 33).getTime()), sender: 'simon', body: 'the retracted words', deleted: true };
  api.messages.mockReturnValue(
    page([{ ...msg('a', new Date(2026, 7, 13, 14, 32).getTime()), sender: 'user', body: 'hello there' }, del]),
  );
  ref.setInput('origin', 'irc');
  ref.setInput('id', '7');
  fixture.detectChanges();
  await settle(thread);
  fixture.detectChanges();
  return fixture;
}

const revealBtn = (f: ComponentFixture<Thread>): HTMLButtonElement | null =>
  (f.nativeElement as HTMLElement).querySelector('.msg[data-id="d"] button[aria-label="Show this deleted message"]');

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

describe('Thread composer — drafts, lines and what a send does', () => {
  function open(ref: ComponentRef<Thread>, fixture: ComponentFixture<Thread>, origin: string, id: string): void {
    ref.setInput('origin', origin);
    ref.setInput('id', id);
    fixture.detectChanges();
  }

  it('keeps a draft per conversation, and never carries one into another', () => {
    const { thread, ref, fixture } = setup();
    open(ref, fixture, 'irc', '7');
    thread.composer.draft.set('to seven, half typed');
    open(ref, fixture, 'irc', '8');
    expect(thread.composer.draft()).toBe('');
    thread.composer.draft.set('to eight');
    open(ref, fixture, 'irc', '7');
    expect(thread.composer.draft()).toBe('to seven, half typed');
    open(ref, fixture, 'irc', '8');
    expect(thread.composer.draft()).toBe('to eight');
  });

  it('sends an IRC paragraph as its lines, in order, and keeps what was refused', async () => {
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
    open(ref, fixture, 'irc', '7');
    api.send.mockImplementation((_o: string, _i: string, text: string) =>
      of(text === 'two' ? { sent: false, error: 'refused: flood', archived: false } : { sent: true, error: null, archived: true }),
    );
    thread.composer.draft.set('one\ntwo\n\nthree');
    await thread.composer.send();
    expect(api.send.mock.calls.map((c) => String(c[2]))).toEqual(['one', 'two']);
    expect(thread.composer.draft()).toBe('two\nthree');
    expect(thread.composer.error()).toContain('flood');
    api.send.mockImplementation(() => of({ sent: true, error: null, archived: true }));
    await thread.composer.send();
    expect(api.send.mock.calls.slice(2).map((c) => String(c[2]))).toEqual(['two', 'three']);
    expect(thread.composer.draft()).toBe('');
  });

  it('sends a Telegram message whole, line breaks and all', async () => {
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
    open(ref, fixture, 'telegram', '4242');
    thread.composer.draft.set('first line\nsecond line');
    await thread.composer.send();
    expect(api.send.mock.calls.map((c) => String(c[2]))).toEqual(['first line\nsecond line']);
  });

  it('merges the newest page in after a send rather than reloading the thread', async () => {
    const { thread, ref, fixture } = setup();
    open(ref, fixture, 'irc', '7');
    await settle(thread);
    const poll = vi.spyOn(thread, 'pollNewer').mockResolvedValue();
    const api = fakeApiOf();
    const loads = api.messages.mock.calls.length;
    thread.composer.draft.set('hello');
    await thread.composer.send();
    expect(poll).toHaveBeenCalledTimes(1);
    expect(api.messages.mock.calls.length).toBe(loads);
  });

  it('Enter sends at a desk and breaks the line on a touch screen', async () => {
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
    open(ref, fixture, 'irc', '7');
    thread.composer.draft.set('a line');
    const enter = (): KeyboardEvent => new KeyboardEvent('keydown', { key: 'Enter', cancelable: true });
    vi.stubGlobal('matchMedia', () => ({ matches: true }));
    const onPhone = enter();
    thread.composer.onKey(onPhone);
    expect(onPhone.defaultPrevented).toBe(false);
    expect(api.send).not.toHaveBeenCalled();
    vi.stubGlobal('matchMedia', () => ({ matches: false }));
    const atDesk = enter();
    thread.composer.onKey(atDesk);
    expect(atDesk.defaultPrevented).toBe(true);
    await new Promise((r) => setTimeout(r, 0));
    expect(api.send).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
  });

  it('leaves the box enabled while sending, so the keyboard stays up', async () => {
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
    api.send.mockReturnValue(new Subject());
    open(ref, fixture, 'irc', '7');
    await settle(thread);
    thread.composer.draft.set('still going');
    void thread.composer.send();
    fixture.detectChanges();
    const root = fixture.nativeElement as HTMLElement;
    expect(thread.composer.sending()).toBe(true);
    expect(root.querySelector<HTMLTextAreaElement>('textarea[name=draft]')!.disabled).toBe(false);
    expect(root.querySelector<HTMLButtonElement>('button[aria-label=Send]')!.disabled).toBe(true);
  });
});

describe('Thread — the page the worker kept', () => {
  async function openKept(network: Subject<MessagesPage>, kept: MessagesPage | null): Promise<{ thread: Thread; fixture: ComponentFixture<Thread> }> {
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
    api.messages.mockReturnValue(network);
    api.keptMessages.mockResolvedValue(kept);
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();
    for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
    fixture.detectChanges();
    return { thread, fixture };
  }
  const kept = (): MessagesPage => ({ messages: [{ ...msg('k', 1000), body: 'kept' }], has_more: false, next_cursor: null, prev_cursor: null });

  it('shows the kept page under the bar before the network answers; the same page then changes nothing', async () => {
    const network = new Subject<MessagesPage>();
    const { thread, fixture } = await openKept(network, kept());
    const root = fixture.nativeElement as HTMLElement;
    expect(thread.rendered().map((m) => m.id)).toEqual(['k']);
    expect(thread.loadingThread()).toBe(true);
    expect(root.querySelector('.loading-thread')).not.toBeNull();
    expect(root.querySelector('.msg[data-id="k"]')).not.toBeNull();
    const before = thread.messages();
    network.next(kept());
    network.complete();
    await new Promise((r) => setTimeout(r, 0));
    fixture.detectChanges();
    expect(thread.loadingThread()).toBe(false);
    expect(thread.messages()).toBe(before);
    expect(root.querySelector('.loading-thread')).toBeNull();
  });

  it('a different page from the network replaces the kept one', async () => {
    const network = new Subject<MessagesPage>();
    const { thread } = await openKept(network, kept());
    network.next({ messages: [{ ...msg('k', 1000), body: 'kept' }, { ...msg('n', 2000), body: 'new' }], has_more: false, next_cursor: null, prev_cursor: null });
    network.complete();
    await new Promise((r) => setTimeout(r, 0));
    expect(thread.rendered().map((m) => m.id)).toEqual(['k', 'n']);
    expect(thread.loadingThread()).toBe(false);
  });

  it('with nothing kept, the bar stands alone until the page comes', async () => {
    const network = new Subject<MessagesPage>();
    const { thread, fixture } = await openKept(network, null);
    const root = fixture.nativeElement as HTMLElement;
    expect(thread.loadingThread()).toBe(true);
    expect(root.querySelector('.loading-thread')).not.toBeNull();
    expect(root.querySelector('.messages')).toBeNull();
  });
});

describe('Thread floating day', () => {
  it('names the day of the message at the top while scrolling, and lets go after a pause', async () => {
    const f = await threeRendered();
    const thread = f.componentInstance;
    expect(thread.floatingDay()).toBeNull();
    expect(thread.scrolling()).toBe(false);
    (f.nativeElement as HTMLElement).dispatchEvent(new Event('scroll'));
    f.detectChanges();
    expect(thread.scrolling()).toBe(true);
    // jsdom lays nothing out, so the first rendered message is the top one.
    expect(thread.floatingDay()).toBe(thread.rendered()[0].ts);
    expect((f.nativeElement as HTMLElement).querySelector('.day.floating.shown')).not.toBeNull();
    await new Promise((r) => setTimeout(r, 1400));
    f.detectChanges();
    expect(thread.scrolling()).toBe(false);
    expect((f.nativeElement as HTMLElement).querySelector('.day.floating.shown')).toBeNull();
  });
});

describe('Thread picking messages by hand', () => {
  const bubble = (f: ComponentFixture<Thread>, id: string): HTMLElement =>
    (f.nativeElement as HTMLElement).querySelector(`.msg[data-id="${id}"]`)!;
  /** The long press: `contextmenu`, as a touch screen and a right click fire it. */
  const press = (f: ComponentFixture<Thread>, on: Element): boolean => {
    const ev = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
    on.dispatchEvent(ev);
    f.detectChanges();
    return ev.defaultPrevented;
  };
  const tap = (f: ComponentFixture<Thread>, on: Element): void => {
    on.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    f.detectChanges();
  };
  const pickedIds = (f: ComponentFixture<Thread>): string[] => f.componentInstance.picking.picked().map((m) => m.id);
  const writes: string[] = [];
  Object.defineProperty(navigator, 'clipboard', {
    value: {
      writeText: (t: string): Promise<void> => {
        writes.push(t);
        return Promise.resolve();
      },
    },
    configurable: true,
  });
  const root = (f: ComponentFixture<Thread>): HTMLElement => f.nativeElement as HTMLElement;

  it('a long press picks the message; taps pick and unpick; unpicking the last leaves', async () => {
    const f = await threeRendered();
    const place = TestBed.inject(Place);
    expect(press(f, bubble(f, 'a'))).toBe(true);
    expect(pickedIds(f)).toEqual(['a']);
    expect(place.context()).toBeDefined();
    expect(root(f).querySelector('.messages.picking')).not.toBeNull();
    tap(f, bubble(f, 'c').querySelector('.body')!);
    expect(pickedIds(f)).toEqual(['a', 'c']);
    tap(f, bubble(f, 'a'));
    expect(pickedIds(f)).toEqual(['c']);
    tap(f, bubble(f, 'c'));
    expect(pickedIds(f)).toEqual([]);
    expect(place.context()).toBeUndefined();
    expect(root(f).querySelector('.messages.picking')).toBeNull();
  });

  it('a further long press, or a Shift-tap, picks the range from the last pick', async () => {
    const f = await threeRendered();
    press(f, bubble(f, 'a'));
    press(f, bubble(f, 'c'));
    expect(pickedIds(f)).toEqual(['a', 'b', 'c']);
    // A plain tap still carves one out; the next range starts from the last pick.
    tap(f, bubble(f, 'b'));
    tap(f, bubble(f, 'a'));
    expect(pickedIds(f)).toEqual(['c']);
    bubble(f, 'a').dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, shiftKey: true }));
    f.detectChanges();
    expect(pickedIds(f)).toEqual(['a', 'b', 'c']);
    // Backwards too: the range runs either way from the anchor.
    tap(f, bubble(f, 'a'));
    tap(f, bubble(f, 'b'));
    tap(f, bubble(f, 'c'));
    expect(pickedIds(f)).toEqual([]);
    press(f, bubble(f, 'c'));
    press(f, bubble(f, 'a'));
    expect(pickedIds(f)).toEqual(['a', 'b', 'c']);
  });

  it('a tap picks nothing until a long press has', async () => {
    const f = await threeRendered();
    tap(f, bubble(f, 'a'));
    expect(pickedIds(f)).toEqual([]);
  });

  it('copies the picked messages as the log, in thread order, and leaves', async () => {
    const f = await threeRendered();
    press(f, bubble(f, 'c'));
    tap(f, bubble(f, 'a'));
    writes.length = 0;
    await f.componentInstance.copyPicked();
    expect(writes).toEqual([
      '--- Day changed Thu Aug 13 2026\n14:32 <user> hello there\n--- Day changed Fri Aug 14 2026\n09:05 <simon> morning',
    ]);
    expect(pickedIds(f)).toEqual([]);
  });

  it('copies one picked message as its words', async () => {
    const f = await threeRendered();
    press(f, bubble(f, 'b'));
    writes.length = 0;
    await f.componentInstance.copyPicked();
    expect(writes).toEqual(['hi']);
  });

  it('a long press on a link leaves the browser its own menu', async () => {
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
    api.messages.mockReturnValue(page([{ ...msg('a', 1000), body: 'see https://example.com/x' }]));
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();
    await settle(thread);
    fixture.detectChanges();
    const link = bubble(fixture, 'a').querySelector('a')!;
    expect(press(fixture, link)).toBe(false);
    expect(pickedIds(fixture)).toEqual([]);
  });

  it('picking is one step in history: back ends it and stays; ending it otherwise takes the step away', async () => {
    const f = await threeRendered();
    // The TestBed's history is a mock behind Angular's `Location`, which is what
    // `wireBack` drives; its back is immediate.
    const history = TestBed.inject(Location);
    const overlay = (): boolean => {
      const s: unknown = history.getState();
      return typeof s === 'object' && s !== null && 'overlay' in s;
    };
    expect(overlay()).toBe(false);
    press(f, bubble(f, 'a'));
    expect(overlay()).toBe(true);
    // Back: the step goes, and so does the picking, nothing else.
    history.back();
    f.detectChanges();
    expect(pickedIds(f)).toEqual([]);
    expect(overlay()).toBe(false);
    // Closed from the bar: the step is taken away for us.
    press(f, bubble(f, 'a'));
    expect(overlay()).toBe(true);
    f.componentInstance.picking.clear();
    expect(overlay()).toBe(false);
    // That a second session's close steps back once, not once per session, is
    // held by e2e/copy.spec.ts: this mock's back is immediate, so a stale
    // listener finds the step already gone and the double step cannot show.
  });

  it('the scroll depth does not rewrite the URL while picking, and catches up after', async () => {
    const f = await threeRendered();
    const navigate = vi.spyOn(TestBed.inject(Router), 'navigate').mockResolvedValue(true);
    press(f, bubble(f, 'a'));
    f.componentInstance.commitFromParam();
    expect(navigate).not.toHaveBeenCalled();
    f.componentInstance.picking.clear();
    f.componentInstance.commitFromParam();
    expect(navigate).toHaveBeenCalledTimes(1);
    expect(navigate.mock.calls[0][1]).toMatchObject({ replaceUrl: true });
  });

  it('Escape leaves, and so does opening another conversation', async () => {
    const f = await threeRendered();
    press(f, bubble(f, 'a'));
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    f.detectChanges();
    expect(pickedIds(f)).toEqual([]);

    press(f, bubble(f, 'a'));
    expect(pickedIds(f)).toEqual(['a']);
    f.componentRef.setInput('id', '8');
    f.detectChanges();
    expect(f.componentInstance.picking.ids().size).toBe(0);
  });
});

describe('Thread copy — saying what was left out', () => {
  /** The thread as three rendered messages out of a conversation of `total`. */
  async function opened3(total: number): Promise<ComponentFixture<Thread>> {
    const f = await threeRendered();
    TestBed.inject(MessagesStore).conversations.set([
      { origin: 'irc', id: '7', name: '#linux', kind: 'group', network: 'xinutec', message_count: total, last_ts: 1, last: null, unread: 0, avatar: null },
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
  async function composer(): Promise<{ thread: Thread; api: FakeApi }> {
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
    api.messages.mockReturnValue(page([msg('a', 1000)]));
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();
    await settle(thread);
    return { thread, api };
  }

  const enter = (over: KeyboardEventInit = {}): KeyboardEvent =>
    new KeyboardEvent('keydown', { key: 'Enter', cancelable: true, ...over });

  /** While an IME is composing, Enter accepts the candidate rather than sending. */
  it('leaves a composing Enter to the IME rather than answering it', async () => {
    const { thread, api } = await composer();
    thread.composer.draft.set('hello wor');
    const e = enter({ isComposing: true });
    thread.composer.onKey(e);
    expect(api.send).not.toHaveBeenCalled();
    expect(e.defaultPrevented).toBe(false);
  });

  /** `send` itself refuses mid-composition: an Enter the handler ignores still
   *  submits the form. The real composition is driven in e2e/ui-pages.spec.ts. */
  it('refuses to send while composing, whatever route reached send', async () => {
    const { thread, api } = await composer();
    thread.composer.draft.set('hello wor');
    thread.composer.composing.set(true);
    await thread.composer.send();
    expect(api.send).not.toHaveBeenCalled();

    thread.composer.composing.set(false);
    await thread.composer.send();
    expect(api.send).toHaveBeenCalledTimes(1);
  });

  it('sends on a plain Enter', async () => {
    const { thread, api } = await composer();
    thread.composer.draft.set('hello world');
    const e = enter();
    thread.composer.onKey(e);
    expect(api.send).toHaveBeenCalledTimes(1);
    expect(e.defaultPrevented).toBe(true);
  });
});

describe('restoring a saved scroll depth', () => {
  /** A stale `?from` older than anything returned, with the API always claiming
   *  another page. */
  async function openWithFrom(from: string): Promise<{ calls: number }> {
    const fixture = setupWithQuery({ from });
    const api = fakeApiOf();
    // Every page is newer than `from`, so only the bound ends the loop. The
    // supply is finite so that removing the bound fails rather than hangs.
    let n = 1000;
    let left = MAX_RESTORE_PAGES + 5;
    api.messages.mockImplementation(() => page([msg(String(n--), 5000)], left-- > 0, 'c'));
    fixture.componentRef.setInput('origin', 'irc');
    fixture.componentRef.setInput('id', '7');
    fixture.detectChanges();
    const thread = fixture.componentInstance;
    await settle(thread, 200);
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
    const fixture = setupWithQuery({ at });
    const api = fakeApiOf();
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
    await settle(thread);
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
        { provide: MessagesApi, useValue: fakeApi() },
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
    const api = fakeApiOf();
    let loads = 0;
    api.messages.mockImplementation((_o: unknown, _i: unknown, _c?: string, _l?: number, dir?: string) => {
      if (dir === 'at') loads++;
      return page([msg('h', 5000)], true, 'c');
    });
    fixture.componentRef.setInput('origin', 'irc');
    fixture.componentRef.setInput('id', '7');
    fixture.detectChanges();
    const thread = fixture.componentInstance;
    await settle(thread);
    expect(loads).toBe(1);

    qp.next(convertToParamMap({ at: '9000_11' }));
    await settle(thread);
    expect(loads).toBe(2);
  });
});

/** Scrolling forward off a landing. */
describe('growing a landing forwards', () => {
  async function landed(newerPages: Message[][]): Promise<{
    thread: Thread;
    dirs: (string | undefined)[];
  }> {
    const fixture = setupWithQuery({ at: '5000_9' });
    const api = fakeApiOf();
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
    await settle(thread);
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
    const fixture = setupWithQuery({ at: 'not-a-cursor' });
    const api = fakeApiOf();
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
    await settle(thread);

    expect(thread.messages().map((m) => m.id)).toEqual(['newest']);
    // Not floating: this is the newest page.
    expect(thread.floating()).toBe(false);
  });
});

/** The landed message is marked, and the mark clears with `?at` on the first
 *  scroll. */
describe('marking the message that was landed on', () => {
  async function land(): Promise<{ thread: Thread; host: HTMLElement }> {
    const fixture = setupWithQuery({ at: '5000_9' });
    const api = fakeApiOf();
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
    await settle(thread);
    return { thread, host: fixture.nativeElement as HTMLElement };
  }

  it('names the hit, and not the message beside it', async () => {
    const { thread } = await land();
    expect(thread.landedId()).toBe('h');
  });

  it('stops naming it once the reader has moved', async () => {
    const { thread, host } = await land();
    expect(thread.landedId()).toBe('h');
    host.dispatchEvent(new WheelEvent('wheel', { deltaY: -100 }));
    thread.commitFromParam();
    expect(thread.landedId()).toBeNull();
  });

  // The pages loaded around a landing scroll too, with nobody's hand on it: on a
  // phone at once, and the marker was gone before it was seen.
  it('keeps naming it while only the app has scrolled', async () => {
    const { thread } = await land();
    thread.commitFromParam();
    expect(thread.landedId()).toBe('h');
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
    const api = fakeApiOf();
    api.conversations.mockReturnValue(of(conversations));
    api.messages.mockReturnValue(page(messages));
    TestBed.inject(MessagesStore).refresh();
    ref.setInput('origin', 'signal');
    ref.setInput('id', 'dm:a');
    fixture.detectChanges();
    await settle(thread);
    fixture.detectChanges();
    const root = fixture.nativeElement as HTMLElement;
    return (id: string) => root.querySelector(`.msg[data-id="${id}"] .tag.delivery`);
  }

  const dm: Conversation = {
    origin: 'signal', id: 'dm:a', name: 'Alice', kind: 'dm',
    network: null, message_count: 1, last_ts: 1000, last: null, unread: 0, avatar: null,
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

/** A bubble names its sender once per run, and only where the bar does not. */
describe('the sender line', () => {
  async function render(messages: Message[], kind: Conversation['kind']) {
    const { thread, ref, fixture } = setup();
    const api = fakeApiOf();
    api.conversations.mockReturnValue(
      of([{ origin: 'signal', id: 'dm:a', name: 'Chat', kind, network: null, message_count: 1, last_ts: 1, last: null, unread: 0, avatar: null }]),
    );
    api.messages.mockReturnValue(page(messages));
    TestBed.inject(MessagesStore).refresh();
    ref.setInput('origin', 'signal');
    ref.setInput('id', 'dm:a');
    fixture.detectChanges();
    await settle(thread);
    fixture.detectChanges();
    const root = fixture.nativeElement as HTMLElement;
    return [...root.querySelectorAll('.msg')].map((el) => el.querySelector('.who')?.textContent?.trim() ?? null);
  }
  const from = (id: string, ts: number, sender: string, is_outgoing = false): Message => ({
    ...msg(id, ts), sender, is_outgoing,
  });

  it('names each run of another sender in a group, once', async () => {
    const named = await render(
      [from('1', 100, 'Alice'), from('2', 200, 'Alice'), from('3', 300, 'Bob'), from('4', 400, 'Me', true), from('5', 500, 'Alice')],
      'group',
    );
    expect(named).toEqual(['Alice', null, 'Bob', null, 'Alice']);
  });

  it('names nobody in a DM', async () => {
    expect(await render([from('1', 100, 'Alice'), from('2', 200, 'Me', true)], 'dm')).toEqual([null, null]);
  });
});
