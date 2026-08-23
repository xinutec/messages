// Turning selected messages into a chat log for the clipboard.
//
// Pure: messages in, text out. The DOM's only job is to say WHICH messages are
// selected (thread.ts); everything printed here comes from the model. So a
// template change cannot alter what gets copied, and the whole format is
// testable without a browser.
//
// ⚠ **The format is irssi's, and it was measured rather than chosen.**
// `signal/src/irclog.rs` classifies 966,039 real log lines and is the importer
// this archive is built on; this is its inverse for the two classes a viewer can
// produce. Keep them agreeing — a log copied out of here should read like a log
// that went in.
//
//     --- Day changed Thu Aug 13 2026
//     14:32 <pippijn> hello there
//     14:35  * pippijn waves
//
// ⚠ **An action's two spaces are irssi's, not a typo.** `HH:MM  * nick text`
// against `HH:MM <nick> text` — the parser matches on it, so losing one space
// makes the line unrecognisable to the thing this is the inverse of.

import { Attachment, Message } from './models';

const pad2 = (n: number): string => String(n).padStart(2, '0');

/** irssi's `%H:%M`, in local time — the log must say what the screen said. */
const hhmm = (d: Date): string => `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;

/** irssi's `%a %b %d %Y` under the C locale, which is exactly what
 *  `toDateString` produces, in every locale and with the same zero-padded day.
 *  Deliberately not the `date` pipe: that follows the app's locale, and the
 *  marker has to keep the one shape the importer parses. */
const dayMarker = (d: Date): string => `--- Day changed ${d.toDateString()}`;

/** An EMPTY name is as good as a missing one — `[image: ]` names nothing. This
 *  is what `thread.html`'s `||` chain does; spelt out because `??` would keep
 *  the empty string and the difference is invisible until it happens. */
const named = (s: string | null): string | null => (s != null && s !== '' ? s : null);

/** What an attachment is, and what it is called when that adds anything.
 *
 *  ⚠ The content type stands in for a MISSING FILENAME, and only when it says
 *  more than the first word already did. A nameless jpeg printed
 *  `[image: image/jpeg]` — the word twice — and it reached a real paste before
 *  anyone noticed, because on screen that branch does not exist: `thread.html`
 *  renders the picture, and falls back to the type only for the file it cannot
 *  show. */
function attachmentLine(a: Attachment): string {
  const what = a.is_image ? 'image' : 'attachment';
  const name = named(a.file_name) ?? (a.is_image ? null : named(a.content_type));
  const label = name != null ? `${what}: ${name}` : what;
  return a.available ? `[${label}]` : `[${label} (not stored)]`;
}

/** One message's text, as the lines it occupies. */
function bodyLines(m: Message): string[] {
  const lines: string[] = [];
  // `thread.html` gates the body on `m.body` and only then swaps in
  // `(deleted)`, so a deleted message with no body shows nothing at all.
  if (m.body) lines.push(...(m.deleted ? ['(deleted)'] : m.body.split('\n')));
  for (const a of m.attachments) lines.push(attachmentLine(a));
  // On the last line, so it reads as a note about the message rather than about
  // its first line.
  if (m.edited && lines.length > 0) lines[lines.length - 1] += ' (edited)';
  return lines;
}

/**
 * Selected messages as an irssi-style log, ready for the clipboard.
 *
 * A day marker precedes the first message as well as every day change: a
 * fragment pasted elsewhere otherwise carries a clock and no date.
 *
 * Each line repeats the timestamp and nick, including the continuation lines of
 * a multi-line Signal message. IRC has no multi-line message, so that is what a
 * real log of the same content looks like — and it keeps any single copied line
 * self-describing, where a bare continuation reads as a new speaker.
 *
 * Reactions are left out. They are not something anybody said, and a log has
 * nowhere to put them.
 */
export function formatChatLog(messages: readonly Message[]): string {
  const lines: string[] = [];
  let day: string | null = null;
  for (const m of messages) {
    const at = new Date(m.ts);
    const marker = dayMarker(at);
    if (marker !== day) {
      lines.push(marker);
      day = marker;
    }
    // An action's sender sits inside the text rather than in brackets.
    const prefix =
      m.kind === 'action' ? `${hhmm(at)}  * ${m.sender} ` : `${hhmm(at)} <${m.sender}> `;
    for (const line of bodyLines(m)) lines.push(prefix + line);
  }
  return lines.join('\n');
}

/**
 * The same log as `text/html`, for targets that paste rich text.
 *
 * `<pre>` rather than markup per field: the alignment IS the format, and Slack,
 * Gmail and Docs all keep a `pre` monospaced instead of reflowing it into prose.
 * Escaping is not cosmetic here — a nick is written `<nick>`, so every line
 * would otherwise paste as an unknown empty tag.
 */
export function chatLogHtml(text: string): string {
  const escaped = text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
  return `<pre>${escaped}</pre>`;
}
