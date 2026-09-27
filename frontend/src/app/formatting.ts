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
  // `?.`: a message without the field must not blank the whole thread.
  if (!m.entities?.length) return [{ text: body, cls: '', href: null }];
  const spans = m.entities
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
  if (e.kind === 'url') return text;
  if (e.kind === 'email') return `mailto:${text}`;
  return null;
}
