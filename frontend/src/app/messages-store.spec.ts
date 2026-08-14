import { provideZonelessChangeDetection } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { of, throwError } from 'rxjs';
import { describe, expect, it, vi } from 'vitest';

import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
import { Conversation } from './models';

const ONE: Conversation[] = [
  { origin: 'irc', id: '7', name: '#chan', kind: 'group', message_count: 1, last_ts: 100 },
];
const TWO: Conversation[] = [
  { origin: 'irc', id: '7', name: '#chan', kind: 'group', message_count: 2, last_ts: 200 },
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

    // ⚠ THE BUG THIS PINS. `init` is idempotent so the shell can call it freely,
    // and that guard used to cover the fetch as well — so the list was read once
    // per app lifetime. Measured against the live archive: the app showed 14,435
    // messages while the database held 14,438, for a message the user had just
    // sent himself.
    api.conversations.mockReturnValue(of(TWO));
    store.refresh();
    expect(store.conversations()[0].message_count).toBe(2);
  });

  it('keeps the list it has when a refresh fails', () => {
    const { store, api } = setup();
    store.init();

    // A stale list beats an empty one: the failure is visible the moment
    // anything is tapped, whereas a list that empties itself on a dropped packet
    // looks like an archive that lost everything.
    api.conversations.mockReturnValue(throwError(() => new Error('offline')));
    store.refresh();
    expect(store.conversations()).toEqual(ONE);
  });
});
