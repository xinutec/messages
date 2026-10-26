// Swiping a message right to answer it, as Signal and Telegram do on a phone.

import { signal } from '@angular/core';

/** How far right a finger must drag a bubble before letting go answers it. */
export const REPLY_AT_PX = 60;
/** How far the bubble follows the finger. */
const FOLLOW_PX = 80;
/** Movement under this is a tap's wobble, not yet a direction. */
const SLOP_PX = 10;

/** One finger on a bubble: undecided, a sideways drag, or a scroll to ignore. */
type Drag =
  | { kind: 'undecided'; id: string; x: number; y: number }
  | { kind: 'sideways'; id: string; x: number }
  | { kind: 'scroll' };

/**
 * A touch that moves right more than it moves up or down drags the bubble with
 * it; let go past `REPLY_AT_PX` and `reply` hears the bubble's id. A mouse never
 * starts one: at a desk a drag selects text. A drag that goes vertical first is
 * a scroll, and stays one.
 */
export class ReplySwipe {
  /** The bubble being dragged and how far, for its `translate`. */
  readonly offset = signal<{ id: string; px: number } | null>(null);
  private drag: Drag | null = null;

  /** The bubble's `translate` while it is dragged, else none. */
  shift(id: string): string | null {
    const o = this.offset();
    return o?.id === id ? `${o.px}px 0` : null;
  }

  constructor(
    /** Whether this message can be answered here. */
    private readonly replyable: (id: string) => boolean,
    private readonly reply: (id: string) => void,
  ) {}

  down(e: PointerEvent): void {
    this.drag = null;
    if (e.pointerType === 'mouse') return;
    const id = messageAt(e);
    if (id == null || !this.replyable(id)) return;
    this.drag = { kind: 'undecided', id, x: e.clientX, y: e.clientY };
  }

  move(e: PointerEvent): void {
    const d = this.drag;
    if (d == null || d.kind === 'scroll') return;
    if (d.kind === 'undecided') {
      const dx = e.clientX - d.x;
      const dy = e.clientY - d.y;
      if (Math.abs(dy) > SLOP_PX && Math.abs(dy) >= Math.abs(dx)) {
        this.drag = { kind: 'scroll' };
        return;
      }
      if (dx <= SLOP_PX) return;
      this.drag = { kind: 'sideways', id: d.id, x: d.x };
    }
    const s = this.drag;
    if (s?.kind !== 'sideways') return;
    this.offset.set({ id: s.id, px: Math.min(Math.max(e.clientX - s.x, 0), FOLLOW_PX) });
  }

  up(e: PointerEvent): void {
    const d = this.drag;
    this.end();
    if (d?.kind === 'sideways' && e.clientX - d.x >= REPLY_AT_PX) this.reply(d.id);
  }

  /** The browser took the touch, or it left: no reply. */
  end(): void {
    this.drag = null;
    this.offset.set(null);
  }
}

function messageAt(e: Event): string | undefined {
  const at = e.target instanceof Element ? e.target : null;
  return at?.closest<HTMLElement>('.msg[data-id]')?.dataset['id'];
}
