// Which messages a copy takes (copy-log.ts): the ones a drag selection touches,
// by their `data-id` (the DOM says which, the model says what; only the rendered
// window can be dragged over), and the ones picked by hand, Signal's way.

import { type Signal, computed, signal } from '@angular/core';
import { type History, wireBack } from '@xinutec/ui-scaffold';
import { Subject, take } from 'rxjs';

import { Message } from './models';

/**
 * Messages picked by hand: a long press picks the first and enters the mode,
 * taps pick and unpick others, unpicking the last leaves. The long press is the
 * `contextmenu` event, which a touch screen fires on one and a desk on a right
 * click. A further long press, or Shift-click at a desk, picks the range from
 * the last message picked through this one, as a file manager's Shift-click
 * does.
 *
 * Kept as ids; `picked` reads them in thread order from every loaded message,
 * so one collapsed out of the DOM by a long scroll is still picked. Picking is
 * one overlay to history (`wireBack`, as a dialog is): starting it adds the
 * entry back spends itself on, and ending it any other way takes the entry
 * away. The owner clears on back (a popstate), and hears `ended` however it
 * ends.
 */
export class Picking {
  readonly ids = signal<ReadonlySet<string>>(new Set());
  readonly picked = computed(() => {
    const ids = this.ids();
    return this.messages().filter((m) => ids.has(m.id));
  });
  /** The last message picked: where a range starts. */
  private anchor: string | null = null;
  /** Picking ended other than by back; see `set`. */
  private readonly closed = new Subject<void>();

  constructor(
    private readonly messages: Signal<readonly Message[]>,
    private readonly history: History,
    private readonly ended: () => void,
  ) {}

  clear(): void {
    this.anchor = null;
    if (this.ids().size > 0) this.set(new Set());
  }

  /** The long press. A link or a picture keeps the browser's own menu. */
  press(e: MouseEvent): void {
    if (e.target instanceof Element && e.target.closest('a, img, video, audio')) return;
    const id = messageAt(e);
    if (id == null) return;
    e.preventDefault();
    // A touch screen's long press may have started selecting the words too.
    document.getSelection()?.removeAllRanges();
    if (this.picked().length > 0) this.through(id);
    else this.toggle(id);
  }

  /** While picking, a tap anywhere on a bubble is a pick; thread.scss keeps
   *  what is inside it from taking the tap. */
  tap(e: MouseEvent): void {
    if (this.picked().length === 0) return;
    const id = messageAt(e);
    if (id == null) return;
    e.preventDefault();
    if (e.shiftKey) this.through(id);
    else this.toggle(id);
  }

  /** Every change goes through here, for the history entry. */
  private set(next: ReadonlySet<string>): void {
    const was = this.ids().size > 0;
    this.ids.set(next);
    const is = next.size > 0;
    // One emission, as a dialog's `afterClosed`: `wireBack` keeps its listener
    // until the first, and a second session must not leave the first's behind.
    if (is && !was) wireBack(this.history, this.closed.pipe(take(1)));
    else if (was && !is) {
      this.closed.next();
      this.ended();
    }
  }

  private toggle(id: string): void {
    const next = new Set(this.ids());
    if (!next.delete(id)) {
      next.add(id);
      this.anchor = id;
    }
    this.set(next);
  }

  /** Pick from the anchor through `id`, both included; a lone `id` when there
   *  is no anchor on view. */
  private through(id: string): void {
    const ids = this.messages().map((m) => m.id);
    const from = this.anchor == null ? -1 : ids.indexOf(this.anchor);
    const to = ids.indexOf(id);
    if (from < 0 || to < 0) {
      if (!this.ids().has(id)) this.toggle(id);
      return;
    }
    const next = new Set(this.ids());
    for (const i of ids.slice(Math.min(from, to), Math.max(from, to) + 1)) next.add(i);
    this.set(next);
    this.anchor = id;
  }
}

function messageAt(e: Event): string | undefined {
  const at = e.target instanceof Element ? e.target : null;
  return at?.closest<HTMLElement>('.msg[data-id]')?.dataset['id'];
}

export function selectedMessages(
  el: HTMLElement | undefined,
  sel: Selection | null,
  rendered: readonly Message[],
): Message[] {
  if (el == null || sel == null || sel.isCollapsed) return [];
  // Firefox allows several ranges.
  const ranges = Array.from({ length: sel.rangeCount }, (_, i) => sel.getRangeAt(i));
  const ids = new Set<string>();
  for (const node of el.querySelectorAll<HTMLElement>('.msg[data-id]')) {
    // `intersectsNode`, not `containsNode`: a selection inside one bubble
    // selects that message. jsdom gets both wrong; e2e/copy.spec.ts covers it.
    if (ranges.some((r) => r.intersectsNode(node))) {
      const id = node.dataset['id'];
      if (id != null) ids.add(id);
    }
  }
  return rendered.filter((m) => ids.has(m.id));
}
