// The one stand-in for `MessagesApi` in unit specs. `satisfies` over the class's
// keys makes a method added to the API a compile error here until it has a stub,
// so no spec calls into `undefined` (six did, when `keptConversations` arrived).
// Each stub carries the method's own signature, so `mockImplementation` and
// `mock.calls` are typed as the real one.

import { TestBed } from '@angular/core/testing';
import { of } from 'rxjs';
import { type Mock, vi } from 'vitest';

import { MessagesApi } from './messages-api';
import { Conversation, Me, MessagesPage, SearchHit } from './models';
import { testMessage } from './test-message';

export type FakeApi = { [K in keyof MessagesApi]: Mock<MessagesApi[K]> };

export const FAKE_ME: Me = { user_id: 'u1', display_name: 'Test User' };

/** A signed-in reader with one conversation page of one message; `over`
 *  replaces any stub. The three request methods answer nothing until a spec
 *  says what they answer. */
export function fakeApi(over: Partial<FakeApi> = {}): FakeApi {
  const page: MessagesPage = {
    messages: [testMessage({ id: '1', ts: 100 })],
    has_more: false,
    next_cursor: null,
    prev_cursor: null,
  };
  const defaults = {
    me: vi.fn<MessagesApi['me']>(() => of(FAKE_ME)),
    logout: vi.fn<MessagesApi['logout']>(() => of({})),
    requestLinkImage: vi.fn<MessagesApi['requestLinkImage']>(),
    telegramMediaState: vi.fn<MessagesApi['telegramMediaState']>(),
    requestTelegramMedia: vi.fn<MessagesApi['requestTelegramMedia']>(),
    conversations: vi.fn<MessagesApi['conversations']>(() => of([] as Conversation[])),
    keptConversations: vi.fn<MessagesApi['keptConversations']>(() => Promise.resolve(null)),
    keptMessages: vi.fn<MessagesApi['keptMessages']>(() => Promise.resolve(null)),
    messages: vi.fn<MessagesApi['messages']>(() => of(page)),
    send: vi.fn<MessagesApi['send']>(() => of({ sent: true, error: null, archived: true, note: null })),
    search: vi.fn<MessagesApi['search']>(() => of([] as SearchHit[])),
  } satisfies { [K in keyof MessagesApi]: unknown };
  return { ...defaults, ...over };
}

/** The fake the TestBed was given, by the class token it stands in for. */
export function fakeApiOf(): FakeApi {
  // eslint-disable-next-line @typescript-eslint/no-unsafe-type-assertion -- the TestBed holds the fake under the class token; this is the one place that says so
  return TestBed.inject(MessagesApi) as unknown as FakeApi;
}
