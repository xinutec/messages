// A message body cut into formatted runs, from the entities Telegram and Signal
// send beside it. Pure: `thread.html` draws the runs.

import { Message } from './models';

/** One run of a body: its text, a class per entity covering it, and the address
 *  of a covering link. */
export interface Segment {
  readonly text: string;
  readonly cls: string;
  readonly href: string | null;
}

/** Cut a body into runs at every entity boundary; each run carries the classes
 *  of every entity covering it, and the address of a covering link. Offsets are
 *  UTF-16 code units, which is what JavaScript indexes by. Runs past the end
 *  are clamped, and the text always comes from `body`, never the entity. */
export function segments(m: Message): Segment[] {
  const body = m.body ?? '';
  // `??`: a message without the field must not blank the whole thread.
  const marked = m.entities ?? [];
  const links = marked.filter((e) => hrefOf(e, body) != null);
  // Addresses the sender's app did not mark: all of them, outside Telegram.
  const found = addresses(body).filter(
    (a) => !links.some((e) => a.offset < e.offset + e.length && e.offset < a.offset + a.length),
  );
  const entities = [...marked, ...found];
  if (!entities.length) return [{ text: body, cls: '', href: null }];
  const spans = entities
    .map((e) => {
      const start = Number(e.offset);
      const stop = Math.min(start + Number(e.length), body.length);
      return { e, start, stop };
    })
    .filter(({ start, stop }) => Number.isFinite(start) && start >= 0 && stop > start);
  const cuts = [...new Set([0, body.length, ...spans.flatMap((s) => [s.start, s.stop])])].sort(
    (a, b) => a - b,
  );
  const out: Segment[] = [];
  for (let i = 0; i + 1 < cuts.length; i++) {
    const [a, b] = [cuts[i], cuts[i + 1]];
    const covering = spans.filter((s) => s.start <= a && s.stop >= b);
    const link = covering.find((s) => hrefOf(s.e, body) != null);
    out.push({
      text: body.slice(a, b),
      cls: covering.map((s) => `fmt-${s.e.kind}`).join(' '),
      href: link ? hrefOf(link.e, body) : null,
    });
  }
  return out;
}

/** Where a link entity points, or null. Reaches the DOM only through the
 *  sanitising `[href]`. */
function hrefOf(e: Message['entities'][number], body: string): string | null {
  const text = body.slice(Number(e.offset), Number(e.offset) + Number(e.length));
  if (e.kind === 'textUrl') return e.url;
  // `www.` alone is an address too; over https, as a browser would try.
  if (e.kind === 'url') return text.startsWith('www.') ? `https://${text}` : text;
  if (e.kind === 'email') return `mailto:${text}`;
  return null;
}

/** Where a plain-text address starts: a web scheme, or `www.`. */
const ADDRESS = /\b(?:https?:\/\/|www\.)[^\s<>"]+/gi;

/** The addresses in a body, as `url` entities. */
function addresses(body: string): Message['entities'] {
  return [...body.matchAll(ADDRESS)].map((match) => ({
    kind: 'url',
    offset: match.index,
    length: trimmed(match[0]).length,
    url: null,
  }));
}

/** An address without what the sentence put after it: closing punctuation, and
 *  a bracket the address itself did not open, as in `(see https://x.org)`. */
function trimmed(address: string): string {
  const count = (s: string, c: string) => s.split(c).length - 1;
  let s = address;
  for (;;) {
    const last = s.at(-1);
    if (last === undefined) return s;
    if ('.,;:!?\'"'.includes(last)) s = s.slice(0, -1);
    else if (last === ')' && count(s, ')') > count(s, '(')) s = s.slice(0, -1);
    else return s;
  }
}
