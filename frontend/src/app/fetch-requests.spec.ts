import { signal } from '@angular/core';
import { of } from 'rxjs';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { FetchRequests } from './fetch-requests';
import { MessagesApi } from './messages-api';
import { Attachment, Message } from './models';
import { fakeApi } from './test-api';
import { testMessage } from './test-message';

const offered: Attachment = {
  id: '7',
  content_type: 'video/mp4',
  file_name: null,
  size: 50_000_000,
  available: false,
  fetch: 'offered',
  transcript: null,
};

describe('a requested Telegram file', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('lands on its message alone; every other message stays the same object', () => {
    const api = fakeApi({
      requestTelegramMedia: vi.fn<MessagesApi['requestTelegramMedia']>(() => of(undefined)),
      telegramMediaState: vi.fn<MessagesApi['telegramMediaState']>(() =>
        of({ available: true, fetch: null, content_type: 'video/mp4' }),
      ),
    });
    const before = testMessage({ id: '6', ts: 1 });
    const holder = testMessage({ id: '7', ts: 2, attachments: [offered] });
    const after = testMessage({ id: '8', ts: 3 });
    const messages = signal<Message[]>([before, holder, after]);
    const fetches = new FetchRequests(api as unknown as MessagesApi, messages);

    fetches.requestMedia(offered);
    vi.advanceTimersByTime(4000);

    const [b, h, a] = messages();
    expect(h.attachments[0].available).toBe(true);
    expect(b).toBe(before);
    expect(a).toBe(after);
  });
});
