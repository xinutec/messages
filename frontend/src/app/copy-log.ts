// Turning selected messages into a chat log for the clipboard. Pure: the DOM
// says which messages are selected (thread.ts); the text comes from the model.
//
// The format is irssi's, the inverse of `archiver/src/irclog.rs`, so a copied log
// reads like one that went in:
//
//     --- Day changed Thu Aug 13 2026
//     14:32 <pippijn> hello there
//     14:35  * pippijn waves
//
// An action's two spaces are irssi's; the parser matches on them.

import { attachmentLabel } from './attachment';
import { Attachment, Message } from './models';

const pad2 = (n: number): string => String(n).padStart(2, '0');

/** irssi's `%H:%M`, in local time, as the screen shows. */
const hhmm = (d: Date): string => `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;

/** irssi's `%a %b %d %Y` in the C locale, which is what `toDateString`
 *  produces in every locale; the `date` pipe would follow the app's. */
const dayMarker = (d: Date): string => `--- Day changed ${d.toDateString()}`;

/** An attachment as one bracketed line, named by `attachment.ts` as on screen. */
function attachmentLine(a: Attachment): string {
  const label = attachmentLabel(a);
  return a.available ? `[${label}]` : `[${label} (not stored)]`;
}

/** One message's text, as the lines it occupies. A deleted message is one
 *  `(deleted)` line, covering its attachments too. */
function bodyLines(m: Message): string[] {
  if (m.deleted) return ['(deleted)'];
  const lines: string[] = [];
  if (m.body) lines.push(...m.body.split('\n'));
  for (const a of m.attachments) lines.push(attachmentLine(a));
  // On the last line, as a note about the whole message.
  if (m.edited && lines.length > 0) lines[lines.length - 1] += ' (edited)';
  return lines;
}

/** What the copied messages are a fragment of, for a copy that tried to take
 *  everything; see `thread.ts`. */
export interface LogScope {
  /** Messages in the whole conversation. */
  total: number;
}

/** A line saying the paste is only part of the conversation, which must travel
 *  with the text. A `--- ` marker, irssi's shape for a line nobody said. */
const omissionNotice = (copied: number, total: number): string =>
  `--- copied ${copied} of ${total} messages; the rest were not loaded on screen`;

/**
 * Selected messages as an irssi-style log. A day marker precedes the first
 * message and every day change. Each line repeats time and nick, including a
 * multi-line message's continuations, so any line stands alone. Reactions are
 * left out.
 */
export function formatChatLog(messages: readonly Message[], scope?: LogScope): string {
  const lines: string[] = [];
  let day: string | null = null;
  for (const m of messages) {
    const at = new Date(m.ts);
    const marker = dayMarker(at);
    if (marker !== day) {
      lines.push(marker);
      day = marker;
    }
    // An action's sender is inside the text.
    const prefix =
      m.kind === 'action' ? `${hhmm(at)}  * ${m.sender} ` : `${hhmm(at)} <${m.sender}> `;
    for (const line of bodyLines(m)) lines.push(prefix + line);
  }
  // `<`: a stale total smaller than what we hold says nothing.
  if (scope != null && messages.length < scope.total) {
    lines.push(omissionNotice(messages.length, scope.total));
  }
  return lines.join('\n');
}

/**
 * The same log as `text/html`: a `<pre>`, which rich-text targets keep
 * monospaced, with every `<nick>` escaped.
 */
export function chatLogHtml(text: string): string {
  const escaped = text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
  return `<pre>${escaped}</pre>`;
}
