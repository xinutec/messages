import { describe, expect, it } from 'vitest';

import { chatLogHtml, formatChatLog } from './copy-log';
import { Attachment, Message } from './models';
import { testMessage } from './test-message';

/** Local time, as the screen shows. */
function at(y: number, mo: number, d: number, h: number, mi: number): number {
  return new Date(y, mo - 1, d, h, mi).getTime();
}

function msg(over: Partial<Message> & Pick<Message, 'ts' | 'sender'>): Message {
  return testMessage({ id: String(over.ts), body: null, ...over });
}

function file(over: Partial<Attachment>): Attachment {
  return {
    id: 'a1',
    content_type: null,
    file_name: null,
    size: null,
    available: true,
    is_image: false,
    // Signal: nothing to ask for. Telegram's states are tested in
    // tests/archive.rs.
    fetch: null,
    ...over,
  };
}

describe('formatChatLog', () => {
  it('writes irssi log lines', () => {
    const out = formatChatLog([
      msg({ ts: at(2026, 8, 13, 14, 32), sender: 'pippijn', body: 'hello there' }),
      msg({ ts: at(2026, 8, 13, 14, 33), sender: 'simon', body: 'hi' }),
    ]);
    expect(out).toBe(
      '--- Day changed Thu Aug 13 2026\n' + //
        '14:32 <pippijn> hello there\n' +
        '14:33 <simon> hi',
    );
  });

  it('dates the first line even when no day changes within the selection', () => {
    // A pasted fragment would otherwise carry a time and no date.
    const out = formatChatLog([msg({ ts: at(2026, 8, 13, 9, 5), sender: 'a', body: 'x' })]);
    expect(out.split('\n')[0]).toBe('--- Day changed Thu Aug 13 2026');
  });

  it('marks each day change, and only the changes', () => {
    const out = formatChatLog([
      msg({ ts: at(2026, 8, 13, 23, 59), sender: 'a', body: 'late' }),
      msg({ ts: at(2026, 8, 14, 0, 1), sender: 'b', body: 'early' }),
      msg({ ts: at(2026, 8, 14, 0, 2), sender: 'b', body: 'still' }),
    ]);
    expect(out.split('\n').filter((l) => l.startsWith('---'))).toEqual([
      '--- Day changed Thu Aug 13 2026',
      '--- Day changed Fri Aug 14 2026',
    ]);
  });

  it('writes an action irssi\'s way, with the sender inside the text', () => {
    const out = formatChatLog([
      msg({ ts: at(2026, 8, 13, 14, 32), sender: 'pippijn', body: 'hello' }),
      msg({ ts: at(2026, 8, 13, 14, 35), sender: 'pippijn', kind: 'action', body: 'waves' }),
    ]);
    expect(out.split('\n').slice(1)).toEqual([
      '14:32 <pippijn> hello', //
      '14:35  * pippijn waves',
    ]);
  });

  it('keeps both of an action line\'s spaces', () => {
    // The parser matches on the double space.
    const out = formatChatLog([
      msg({ ts: at(2026, 8, 13, 14, 35), sender: 'p', kind: 'action', body: 'waves' }),
    ]);
    expect(out).toContain('14:35  * p waves');
    expect(out).not.toContain('14:35 * p waves');
  });

  it('zero-pads the clock', () => {
    const out = formatChatLog([msg({ ts: at(2026, 8, 13, 7, 4), sender: 'a', body: 'x' })]);
    expect(out).toContain('07:04 <a> x');
  });

  it('repeats the prefix on every line of a multi-line body', () => {
    // Each line stands alone, as in a real IRC log.
    const out = formatChatLog([
      msg({ ts: at(2026, 8, 13, 14, 32), sender: 'pippijn', body: 'one\ntwo' }),
    ]);
    expect(out).toBe(
      '--- Day changed Thu Aug 13 2026\n' + //
        '14:32 <pippijn> one\n' +
        '14:32 <pippijn> two',
    );
  });

  it('says (deleted) and (edited) the way the screen does', () => {
    const out = formatChatLog([
      msg({ ts: at(2026, 8, 13, 1, 0), sender: 'a', body: 'gone', deleted: true }),
      msg({ ts: at(2026, 8, 13, 1, 1), sender: 'a', body: 'fixed', edited: true }),
    ]);
    expect(out).toContain('01:00 <a> (deleted)');
    expect(out).toContain('01:01 <a> fixed (edited)');
  });

  /** A deleted message still leaves a line. */
  it('still says (deleted) when the body is gone, so the line is not dropped', () => {
    const out = formatChatLog([msg({ ts: at(2026, 8, 13, 1, 0), sender: 'a', deleted: true })]);
    expect(out).toContain('01:00 <a> (deleted)');
  });

  /** A deletion covers the attachments too. */
  it('does not name the attachments of a deleted message', () => {
    const out = formatChatLog([
      msg({
        ts: at(2026, 8, 13, 1, 0),
        sender: 'a',
        body: 'gone',
        deleted: true,
        attachments: [file({ file_name: 'shot.png', is_image: true })],
      }),
      // Attachment-only: no body at all.
      msg({
        ts: at(2026, 8, 13, 1, 1),
        sender: 'a',
        deleted: true,
        attachments: [file({ file_name: 'secret.jpg', is_image: true })],
      }),
    ]);
    expect(out).not.toContain('shot.png');
    expect(out).not.toContain('secret.jpg');
    expect(out).toContain('01:00 <a> (deleted)');
    expect(out).toContain('01:01 <a> (deleted)');
  });

  it('names attachments, and says which are not stored', () => {
    const out = formatChatLog([
      msg({
        ts: at(2026, 8, 13, 1, 0),
        sender: 'a',
        attachments: [
          file({ file_name: 'shot.png', is_image: true }),
          file({ file_name: 'notes.pdf' }),
          file({ file_name: 'old.jpg', is_image: true, available: false, fetch: null }),
          file({ content_type: 'audio/ogg' }),
          file({ file_name: '', content_type: 'text/plain' }),
          // A Signal photo has no filename; `[image: image/jpeg]` would say
          // image twice.
          file({ content_type: 'image/jpeg', is_image: true }),
          file({ content_type: 'image/png', is_image: true, available: false, fetch: null }),
          file({}),
        ],
      }),
    ]);
    expect(out.split('\n').slice(1)).toEqual([
      '01:00 <a> [image: shot.png]',
      '01:00 <a> [attachment: notes.pdf]',
      '01:00 <a> [image: old.jpg (not stored)]',
      '01:00 <a> [attachment: audio/ogg]',
      '01:00 <a> [attachment: text/plain]',
      '01:00 <a> [image]',
      '01:00 <a> [image (not stored)]',
      // Nothing known: no fallback printed as if it were a name.
      '01:00 <a> [attachment]',
    ]);
  });

  it('puts (edited) after the attachments, not before them', () => {
    const out = formatChatLog([
      msg({
        ts: at(2026, 8, 13, 1, 0),
        sender: 'a',
        body: 'see this',
        edited: true,
        attachments: [file({ file_name: 'shot.png', is_image: true })],
      }),
    ]);
    expect(out.split('\n').slice(1)).toEqual([
      '01:00 <a> see this',
      '01:00 <a> [image: shot.png] (edited)',
    ]);
  });

  it('leaves reactions out — they are furniture, not things anybody said', () => {
    const out = formatChatLog([
      msg({
        ts: at(2026, 8, 13, 1, 0),
        sender: 'a',
        body: 'x',
        reactions: [{ emoji: '👍', count: 2, who: ['a', 'b'] }],
      }),
    ]);
    expect(out).toBe('--- Day changed Thu Aug 13 2026\n01:00 <a> x');
  });

  it('is empty for no messages', () => {
    expect(formatChatLog([])).toBe('');
  });
});

describe('chatLogHtml', () => {
  it('wraps in <pre> so a rich target keeps the alignment', () => {
    expect(chatLogHtml('14:32 <a> hi')).toBe('<pre>14:32 &lt;a&gt; hi</pre>');
  });

  it('escapes what would otherwise be markup', () => {
    // Every `<nick>` must be escaped.
    expect(chatLogHtml('a & <b> "c"')).toBe('<pre>a &amp; &lt;b&gt; &quot;c&quot;</pre>');
  });
});

describe('what the log leaves out', () => {
  const two = [
    msg({ ts: at(2026, 8, 13, 1, 0), sender: 'a', body: 'one' }),
    msg({ ts: at(2026, 8, 13, 1, 1), sender: 'a', body: 'two' }),
  ];

  it('says nothing extra when the copy IS the whole conversation', () => {
    expect(formatChatLog(two, { total: 2 })).not.toContain('not loaded');
  });

  it('says nothing when nobody said how big the conversation is', () => {
    expect(formatChatLog(two)).not.toContain('not loaded');
  });

  /** A select-all copies only the rendered window; the paste says so. */
  it('names what was left out when the window held only part of it', () => {
    const out = formatChatLog(two, { total: 401794 });
    expect(out).toContain('copied 2 of 401794 messages');
    // Last line, as a footnote.
    expect(out.split('\n').at(-1)).toMatch(/^--- copied 2 of 401794 messages/);
  });

  it('leaves the messages themselves untouched', () => {
    const withScope = formatChatLog(two, { total: 99 }).split('\n');
    const without = formatChatLog(two).split('\n');
    expect(withScope.slice(0, without.length)).toEqual(without);
  });
});