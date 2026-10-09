// What a row in the list shows besides its name: the picture stand-in, the time,
// and the newest message. Pure: app.html and avatar.ts draw them.

import { Conversation, Origin } from './models';

/** One label per origin. A `Record`, so a new origin is a type error until
 *  labelled. */
export const ORIGIN_LABELS: Record<Origin, string> = {
  signal: 'Signal',
  gchat: 'Google Chat',
  irc: 'IRC',
  telegram: 'Telegram',
};

/** Up to two letters for the round picture: the first two words' initials,
 *  past an IRC channel's `#`. */
export function initials(name: string): string {
  const words = name.replace(/^[#&@]+/, '').split(/\s+/).filter(Boolean);
  // By code point, so a name starting with an emoji keeps it whole.
  return words.slice(0, 2).map((w) => [...w][0]).join('').toUpperCase() || '?';
}

/** A hue per conversation, the same on every load. */
export function hue(origin: Origin, id: string): number {
  let h = 0;
  for (const ch of origin + id) h = (h * 31 + ch.charCodeAt(0)) % 360;
  return h;
}

/** A `DatePipe` format as short as the age allows: the time today, the weekday
 *  this week, the day this year, else the date. */
export function whenFormat(ts: number, now = new Date()): string {
  const then = new Date(ts);
  const days = Math.round((startOfDay(now) - startOfDay(then)) / 86_400_000);
  if (days < 1) return 'HH:mm';
  if (days < 7) return 'EEE';
  return then.getFullYear() === now.getFullYear() ? 'd MMM' : 'd MMM y';
}

function startOfDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/** The line under the name: who wrote the newest message, when that needs
 *  saying, and what it said. */
export interface Preview {
  readonly who: string | null;
  readonly text: string;
  /** No words to show: deleted, or a message without text. */
  readonly quiet: boolean;
}

export function preview(c: Conversation): Preview | null {
  const last = c.last;
  if (!last) return null;
  // A DM's other side needs no name; a group's sender does.
  const who = last.is_outgoing ? 'You' : c.kind === 'dm' ? null : last.sender;
  if (last.deleted) return { who, text: 'Message deleted', quiet: true };
  if (last.text !== null) return { who, text: last.text, quiet: false };
  if (last.media !== null) return { who, text: carried(last.media), quiet: false };
  return { who, text: 'No text', quiet: true };
}

/** What an attachment is, by its content type, as Signal's list names it. */
function carried(type: string): string {
  if (type.startsWith('image/')) return 'Photo';
  if (type.startsWith('video/')) return 'Video';
  if (type.startsWith('audio/')) return 'Audio';
  return 'File';
}
