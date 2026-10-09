import { describe, expect, it } from 'vitest';

import { segments } from './formatting';
import { Message } from './models';
import { testMessage } from './test-message';

/** Entity offsets are UTF-16 code units, as `String.prototype.slice` counts. */
describe('formatted message bodies', () => {
  function withEntities(body: string, entities: Message['entities']): Message {
    return testMessage({ id: 'm', ts: 1000, body, entities });
  }

  it('splits a body into plain and formatted runs', () => {
    const m = withEntities('hello brave world', [
      { kind: 'bold', offset: 6, length: 5, url: null },
    ]);
    expect(segments(m)).toEqual([
      { text: 'hello ', cls: '', href: null },
      { text: 'brave', cls: 'fmt-bold', href: null },
      { text: ' world', cls: '', href: null },
    ]);
  });

  it('counts an emoji as TWO, because Telegram and Signal do', () => {
    // '👋' is two code units, so offset 2 is after it.
    const m = withEntities('👋 bold', [{ kind: 'bold', offset: 3, length: 4, url: null }]);
    expect(segments(m).map((s) => s.text)).toEqual(['👋 ', 'bold']);
  });

  it('never lets an entity change how much of the message is shown', () => {
    const body = 'short';
    // Past the end: clamped.
    const over = withEntities(body, [{ kind: 'bold', offset: 2, length: 99, url: null }]);
    expect(segments(over).map((s) => s.text).join('')).toBe(body);
  });

  it('combines overlapping runs rather than dropping one', () => {
    // Signal's own shape: one span both monospace and struck, and a nested run.
    const m = withEntities('abcdefgh mono', [
      { kind: 'bold', offset: 0, length: 5, url: null },
      { kind: 'italic', offset: 2, length: 3, url: null },
      { kind: 'code', offset: 9, length: 4, url: null },
      { kind: 'strike', offset: 9, length: 4, url: null },
    ]);
    expect(segments(m)).toEqual([
      { text: 'ab', cls: 'fmt-bold', href: null },
      { text: 'cde', cls: 'fmt-bold fmt-italic', href: null },
      { text: 'fgh ', cls: '', href: null },
      { text: 'mono', cls: 'fmt-code fmt-strike', href: null },
    ]);
  });

  it('keeps a link whole when formatting splits it', () => {
    const m = withEntities('see https://x.org now', [
      { kind: 'url', offset: 4, length: 13, url: null },
      { kind: 'bold', offset: 12, length: 5, url: null },
    ]);
    expect(segments(m)).toEqual([
      { text: 'see ', cls: '', href: null },
      { text: 'https://', cls: 'fmt-url', href: 'https://x.org' },
      { text: 'x.org', cls: 'fmt-url fmt-bold', href: 'https://x.org' },
      { text: ' now', cls: '', href: null },
    ]);
  });

  it('finds an address in plain text, as Signal, IRC and Google Chat send it', () => {
    const m = withEntities('look at https://xinutec.org/a?b=1. ok', []);
    expect(segments(m)).toEqual([
      { text: 'look at ', cls: '', href: null },
      { text: 'https://xinutec.org/a?b=1', cls: 'fmt-url', href: 'https://xinutec.org/a?b=1' },
      { text: '. ok', cls: '', href: null },
    ]);
  });

  it('takes www. without a scheme, over https', () => {
    expect(segments(withEntities('www.example.org!', []))[0]).toEqual({
      text: 'www.example.org', cls: 'fmt-url', href: 'https://www.example.org',
    });
  });

  it('keeps a bracket the address opened, drops one the sentence did', () => {
    const m = withEntities('(see https://en.wikipedia.org/wiki/Foo_(bar))', []);
    expect(segments(m).find((s) => s.href)?.href).toBe('https://en.wikipedia.org/wiki/Foo_(bar)');
  });

  it('adds nothing where the sender already marked the link', () => {
    const m = withEntities('see https://x.org now', [{ kind: 'url', offset: 4, length: 13, url: null }]);
    expect(segments(m).filter((s) => s.href)).toEqual([
      { text: 'https://x.org', cls: 'fmt-url', href: 'https://x.org' },
    ]);
  });

  it('is no link without a scheme or www', () => {
    expect(segments(withEntities('mail me at x.org or ftp://y', []))).toEqual([
      { text: 'mail me at x.org or ftp://y', cls: '', href: null },
    ]);
  });

  it('is exactly the body when nothing is formatted', () => {
    const m = withEntities('just words', []);
    expect(segments(m)).toEqual([{ text: 'just words', cls: '', href: null }]);
  });
});
