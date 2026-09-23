import { provideZonelessChangeDetection } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { of, throwError } from 'rxjs';
import { describe, expect, it, vi } from 'vitest';

import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
import { Conversation } from './models';

const ONE: Conversation[] = [
  { origin: 'irc', id: '7', name: '#chan', kind: 'group', network: 'xinutec', message_count: 1, last_ts: 100 },
];
const TWO: Conversation[] = [
  { origin: 'irc', id: '7', name: '#chan', kind: 'group', network: 'xinutec', message_count: 2, last_ts: 200 },
];

function setup(): { store: MessagesStore; api: { conversations: ReturnType<typeof vi.fn> } } {
  const api = {
    me: vi.fn(() => of({ user_id: 'u1', display_name: 'Test User' })),
    conversations: vi.fn(() => of(ONE)),
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
