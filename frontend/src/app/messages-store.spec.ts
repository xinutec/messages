import { provideZonelessChangeDetection } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { HttpErrorResponse } from '@angular/common/http';
import { Subject, of, throwError } from 'rxjs';
import { describe, expect, it, vi } from 'vitest';

import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
import { Conversation } from './models';

const ONE: Conversation[] = [
  { origin: 'irc', id: '7', name: '#chan', kind: 'group', network: 'xinutec', message_count: 1, last_ts: 100, last: null, unread: 0, avatar: null },
];
const TWO: Conversation[] = [
  { origin: 'irc', id: '7', name: '#chan', kind: 'group', network: 'xinutec', message_count: 2, last_ts: 200, last: null, unread: 0, avatar: null },
];

function setup(): { store: MessagesStore; api: { conversations: ReturnType<typeof vi.fn> } } {
  const api = {
    me: vi.fn(() => of({ user_id: 'u1', display_name: 'Test User' })),
    conversations: vi.fn(() => of(ONE)),
    keptConversations: vi.fn(() => Promise.resolve(null)),
  } as unknown as MessagesApi;
  TestBed.configureTestingModule({
    providers: [provideZonelessChangeDetection(), { provide: MessagesApi, useValue: api }],
  });
  return {
    store: TestBed.inject(MessagesStore),
    api: api as unknown as { conversations: ReturnType<typeof vi.fn> },
  };
}

describe('MessagesStore', () => {
  it('re-reads the list on refresh — init guards the start, not the fetch', () => {
    const { store, api } = setup();
    store.init();
    expect(store.conversations()[0].message_count).toBe(1);

    // `init` is idempotent; `refresh` must still fetch.
    api.conversations.mockReturnValue(of(TWO));
    store.refresh();
    expect(store.conversations()[0].message_count).toBe(2);
  });

  it('keeps the list it has when a refresh fails', () => {
    const { store, api } = setup();
    store.init();

    // A stale list beats an empty one.
    api.conversations.mockReturnValue(throwError(() => new Error('offline')));
    store.refresh();
    expect(store.conversations()).toEqual(ONE);
  });
});

describe('MessagesStore — opening offline', () => {
  const remembered = { user_id: 'u1', display_name: 'Test User' };
  const failing = (status: number) =>
    vi.fn(() => throwError(() => new HttpErrorResponse({ status, statusText: status === 0 ? 'Unknown Error' : 'Error' })));

  function setupWith(me: ReturnType<typeof vi.fn>): { store: MessagesStore; conversations: ReturnType<typeof vi.fn> } {
    const conversations = vi.fn(() => of(ONE));
    TestBed.configureTestingModule({
      providers: [provideZonelessChangeDetection(), { provide: MessagesApi, useValue: { me, conversations, keptConversations: () => Promise.resolve(null) } }],
    });
    return { store: TestBed.inject(MessagesStore), conversations };
  }

  it('a signed-in start is remembered, and a later start with no network opens on it, offline', () => {
    localStorage.clear();
    const first = setupWith(vi.fn(() => of(remembered)));
    first.store.init();
    expect(localStorage.getItem('messages.me')).toContain('u1');

    TestBed.resetTestingModule();
    const { store, conversations } = setupWith(failing(0));
    expect(store.loading()).toBe(false);
    expect(store.me()?.user_id).toBe('u1');
    store.init();
    expect(store.me()?.user_id).toBe('u1');
    expect(store.offline()).toBe(true);
    // The list is still asked for: the service worker may hold it.
    expect(conversations).toHaveBeenCalledTimes(1);
  });

  it('the service worker\'s offline answer (504) is not a sign-out either', () => {
    localStorage.setItem('messages.me', JSON.stringify(remembered));
    const { store } = setupWith(failing(504));
    store.init();
    expect(store.me()?.user_id).toBe('u1');
    expect(store.offline()).toBe(true);
  });

  it('a 401 is a sign-out: the identity goes, and the sign-in page shows', () => {
    localStorage.setItem('messages.me', JSON.stringify(remembered));
    const { store } = setupWith(failing(401));
    store.init();
    expect(store.me()).toBeNull();
    expect(store.offline()).toBe(false);
    expect(localStorage.getItem('messages.me')).toBeNull();
  });

  it('with nothing remembered and no network, it is offline rather than signed out', () => {
    localStorage.clear();
    const { store, conversations } = setupWith(failing(0));
    expect(store.loading()).toBe(true);
    store.init();
    expect(store.loading()).toBe(false);
    expect(store.me()).toBeNull();
    expect(store.offline()).toBe(true);
    expect(conversations).not.toHaveBeenCalled();
  });

  it('forgetting, on logout, clears what was remembered', () => {
    localStorage.setItem('messages.me', JSON.stringify(remembered));
    const { store } = setupWith(vi.fn(() => of(remembered)));
    store.forget();
    expect(store.me()).toBeNull();
    expect(localStorage.getItem('messages.me')).toBeNull();
  });
});

describe('MessagesStore — the list the worker kept', () => {
  it('shows the kept list at once, and the network\'s replaces it, an empty one included', async () => {
    localStorage.setItem('messages.me', JSON.stringify({ user_id: 'u1', display_name: 'Test User' }));
    vi.stubGlobal('caches', {
      match: (url: string) => Promise.resolve(url === '/api/conversations' ? new Response(JSON.stringify(ONE)) : undefined),
    });
    const network = new Subject<Conversation[]>();
    TestBed.configureTestingModule({
      providers: [
        provideZonelessChangeDetection(),
        { provide: MessagesApi, useValue: { me: vi.fn(() => of({ user_id: 'u1', display_name: 'Test User' })), conversations: vi.fn(() => network), keptConversations: () => kept() } },
      ],
    });
    const store = TestBed.inject(MessagesStore);
    store.init();
    await new Promise((r) => setTimeout(r, 0));
    expect(store.conversations()).toEqual(ONE);
    network.next([]);
    expect(store.conversations()).toEqual([]);
    // Late kept data never overwrites what the network said.
    await new Promise((r) => setTimeout(r, 0));
    expect(store.conversations()).toEqual([]);
    vi.unstubAllGlobals();
  });
});

/** The API's reader, through the stubbed Cache API; see messages-api.ts. */
async function kept(): Promise<Conversation[] | null> {
  const res = await caches.match('/api/conversations');
  return res == null ? null : ((await res.json()) as Conversation[]);
}
