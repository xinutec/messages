import { describe, expect, it } from 'vitest';

import { attachmentUrl, deliveryLabel, hostOf, isPlain, mib, reactors } from './message-view';
import { Attachment, Delivery } from './models';
import { testMessage } from './test-message';

const file = { id: '9' } as Attachment;

describe('how a message is shown', () => {
  it('fetches each origin\'s files from its own route', () => {
    expect(attachmentUrl('signal', file)).toBe('/api/attachments/9');
    expect(attachmentUrl('gchat', file)).toBe('/api/gchat-attachments/9');
    expect(attachmentUrl('telegram', file)).toBe('/api/telegram-media/9');
  });

  it('counts reactors it cannot name, and never invents them', () => {
    expect(reactors({ emoji: '👍', count: 3, who: ['Dana'] })).toBe('Dana and 2 more');
    expect(reactors({ emoji: '👍', count: 1, who: ['Dana'] })).toBe('Dana');
    expect(reactors({ emoji: '👍', count: 2, who: [] })).toBe('');
  });

  it('says plain "read" in a DM, and counts readers elsewhere', () => {
    const read: Delivery = { state: 'read', read_by: [{ who: 'A', at: 1 }, { who: 'B', at: 2 }] };
    expect(deliveryLabel(read, 'dm')).toBe('read');
    expect(deliveryLabel(read, 'group')).toBe('read by 2');
    expect(deliveryLabel(read, undefined)).toBe('read by 2');
    expect(deliveryLabel({ state: 'delivered', read_by: [] }, 'group')).toBe('delivered');
  });

  it('rounds a large size and keeps a decimal on a small one', () => {
    expect(mib(14950323)).toBe('14 MB');
    expect(mib(2 * 1024 * 1024 + 300_000)).toBe('2.3 MB');
  });

  it('shows a link\'s host, or the text when it is no URL', () => {
    expect(hostOf('https://xinutec.org/a/b')).toBe('xinutec.org');
    expect(hostOf('not a url')).toBe('not a url');
  });

  it('calls a message plain only when it is words and nothing more', () => {
    expect(isPlain(testMessage({ body: 'hi' }))).toBe(true);
    expect(isPlain(testMessage({ body: 'hi', attachments: [file] }))).toBe(false);
    expect(isPlain(testMessage({ body: 'hi', deleted: true }))).toBe(false);
    expect(isPlain(testMessage({ body: null }))).toBe(false);
  });
});
