// How the thread shows a message, as pure functions: where its files come from,
// what its delivery tag says, who reacted, and how messages group into days and
// runs. Pure, so a spec can ask them directly; `thread.ts` hands them to its
// template.

import { formatDate } from '@angular/common';

import { Attachment, ConversationKind, Delivery, Message, Origin, Reaction } from './models';

/** Where an attachment's bytes come from: each origin has its own route, since
 *  attachment ids are per origin. A Record, so a new origin is a type error
 *  rather than another origin's endpoint. */
const ATTACHMENT_ROUTES: Record<Origin, string> = {
  signal: 'attachments',
  gchat: 'gchat-attachments',
  telegram: 'telegram-media',
  // IRC has no attachments.
  irc: 'attachments',
};

export function attachmentUrl(origin: Origin, a: Attachment): string {
  return `/api/${ATTACHMENT_ROUTES[origin]}/${a.id}`;
}

/** Hover text naming who reacted. `who` can be shorter than `count`; the rest
 *  are counted, never invented. */
export function reactors(r: Reaction): string {
  if (!r.who.length) return '';
  const named = r.who.join(', ');
  const unnamed = r.count - r.who.length;
  return unnamed > 0 ? `${named} and ${unnamed} more` : named;
}

/** The tag on an outgoing message. Plain "read" only where it covers everyone
 *  (a DM, or Telegram's position); where people are named, "read by 2". An
 *  unloaded conversation kind gets the counted form. */
export function deliveryLabel(d: Delivery, kind: ConversationKind | undefined): string {
  if (d.state !== 'read' && d.state !== 'viewed') return d.state;
  if (!d.read_by.length || kind === 'dm') return d.state;
  return `${d.state} by ${d.read_by.length}`;
}

/** Who read it and when; empty for Telegram. `formatDate` with the app's
 *  locale, to match the `| date` beside it. */
export function readers(d: Delivery, locale: string): string {
  return d.read_by.map((r) => `${r.who} ${formatDate(r.at, 'short', locale)}`).join(', ');
}

/** Dimmed until somebody has actually read it. */
export function deliveryPending(d: Delivery): boolean {
  return d.state === 'sent' || d.state === 'delivered';
}

/** A preview's host, or its whole url if that does not parse. */
export function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/** A size for something not yet fetched. */
export function mib(bytes: number): string {
  const mib = bytes / (1024 * 1024);
  return mib >= 10 ? `${Math.round(mib)} MB` : `${mib.toFixed(1)} MB`;
}

/** Words and nothing after them, so the time can share their last line. */
export function isPlain(m: Message): boolean {
  return !!m.body && !m.deleted && !m.previews.length && !m.attachments.length
    && !m.link_images.length && !m.link_offers.length;
}

/** One day of the rendered window, under its sticky date header. */
export interface DayGroup {
  readonly key: string;
  readonly ts: number;
  readonly runs: Message[][];
}

/** Messages grouped by day. Within a day, adjacent members of one album (same
 *  `album`, same sender) form a run, drawn as one set; every other message is a
 *  run of one. */
export function dayGroups(messages: readonly Message[]): DayGroup[] {
  const groups: { key: string; ts: number; runs: Message[][] }[] = [];
  let lastKey: string | null = null;
  for (const m of messages) {
    const key = new Date(m.ts).toDateString();
    if (key !== lastKey) {
      groups.push({ key, ts: m.ts, runs: [] });
      lastKey = key;
    }
    const runs = groups[groups.length - 1].runs;
    const prev = runs.at(-1)?.at(-1);
    if (m.album != null && prev?.album === m.album && prev.sender === m.sender) {
      runs[runs.length - 1].push(m);
    } else {
      runs.push([m]);
    }
  }
  return groups;
}

/** The messages that start a run by one sender on one day: where a bubble names
 *  who speaks, and where the gap between bubbles widens. */
export function runStarts(messages: readonly Message[]): ReadonlySet<string> {
  const starts = new Set<string>();
  let prev: Message | undefined;
  for (const m of messages) {
    const sameDay = prev && new Date(prev.ts).toDateString() === new Date(m.ts).toDateString();
    if (!sameDay || prev?.sender !== m.sender || prev.is_outgoing !== m.is_outgoing) starts.add(m.id);
    prev = m;
  }
  return starts;
}
