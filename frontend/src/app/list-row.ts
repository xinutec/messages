// What a conversation's row shows besides its name: the picture stand-in, the
// time, and the newest message. Pure: app.html draws them.

import { Conversation } from './models';

/** Up to two letters for the round picture: the first two words' initials,
 *  past an IRC channel's `#`. */
export function initials(name: string): string {
  const words = name.replace(/^[#&@]+/, '').split(/\s+/).filter(Boolean);
  // By code point, so a name starting with an emoji keeps it whole.
  return words.slice(0, 2).map((w) => [...w][0]).join('').toUpperCase() || '?';
}

/** A hue per conversation, the same on every load. */
export function hue(c: Conversation): number {
  let h = 0;
  for (const ch of c.origin + c.id) h = (h * 31 + ch.charCodeAt(0)) % 360;
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
  return { who, text: last.text ?? 'No text', quiet: last.text === null };
}
