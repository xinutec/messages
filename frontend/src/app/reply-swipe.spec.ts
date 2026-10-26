import { describe, expect, it, vi } from 'vitest';

import { REPLY_AT_PX, ReplySwipe } from './reply-swipe';

/** A bubble with a child, as a finger lands on what is inside it. */
function bubble(id: string): Element {
  const msg = document.createElement('div');
  msg.className = 'msg';
  msg.dataset['id'] = id;
  const inner = document.createElement('span');
  msg.append(inner);
  return inner;
}

function at(target: Element, clientX: number, clientY: number, pointerType = 'touch'): PointerEvent {
  return { target, clientX, clientY, pointerType } as unknown as PointerEvent;
}

function setup(replyable = true): { swipe: ReplySwipe; reply: ReturnType<typeof vi.fn> } {
  const reply = vi.fn<(id: string) => void>();
  return { swipe: new ReplySwipe(() => replyable, reply), reply };
}

describe('ReplySwipe', () => {
  it('answers a bubble dragged right past the mark, and follows the finger meanwhile', () => {
    const { swipe, reply } = setup();
    const el = bubble('7');
    swipe.down(at(el, 100, 300));
    swipe.move(at(el, 130, 302));
    expect(swipe.offset()).toEqual({ id: '7', px: 30 });
    swipe.move(at(el, 100 + REPLY_AT_PX + 40, 304));
    expect(swipe.offset()).toEqual({ id: '7', px: 80 });
    swipe.up(at(el, 100 + REPLY_AT_PX + 40, 304));
    expect(reply).toHaveBeenCalledWith('7');
    expect(swipe.offset()).toBeNull();
  });

  it('lets go short of the mark without answering', () => {
    const { swipe, reply } = setup();
    const el = bubble('7');
    swipe.down(at(el, 100, 300));
    swipe.move(at(el, 100 + REPLY_AT_PX - 5, 300));
    swipe.up(at(el, 100 + REPLY_AT_PX - 5, 300));
    expect(reply).not.toHaveBeenCalled();
    expect(swipe.offset()).toBeNull();
  });

  it('leaves a scroll alone, even when it later drifts right', () => {
    const { swipe, reply } = setup();
    const el = bubble('7');
    swipe.down(at(el, 100, 300));
    swipe.move(at(el, 102, 340));
    swipe.move(at(el, 100 + REPLY_AT_PX + 40, 360));
    swipe.up(at(el, 100 + REPLY_AT_PX + 40, 360));
    expect(reply).not.toHaveBeenCalled();
    expect(swipe.offset()).toBeNull();
  });

  it('ignores a leftward drag', () => {
    const { swipe, reply } = setup();
    const el = bubble('7');
    swipe.down(at(el, 200, 300));
    swipe.move(at(el, 120, 300));
    swipe.up(at(el, 120, 300));
    expect(reply).not.toHaveBeenCalled();
    expect(swipe.offset()).toBeNull();
  });

  it('never starts from a mouse, whose drag selects text', () => {
    const { swipe, reply } = setup();
    const el = bubble('7');
    swipe.down(at(el, 100, 300, 'mouse'));
    swipe.move(at(el, 200, 300, 'mouse'));
    swipe.up(at(el, 200, 300, 'mouse'));
    expect(reply).not.toHaveBeenCalled();
    expect(swipe.offset()).toBeNull();
  });

  it('does not start on a message that cannot be answered', () => {
    const { swipe, reply } = setup(false);
    const el = bubble('7');
    swipe.down(at(el, 100, 300));
    swipe.move(at(el, 200, 300));
    swipe.up(at(el, 200, 300));
    expect(reply).not.toHaveBeenCalled();
  });

  it('answers nothing when the browser takes the touch', () => {
    const { swipe, reply } = setup();
    const el = bubble('7');
    swipe.down(at(el, 100, 300));
    swipe.move(at(el, 200, 300));
    swipe.end();
    swipe.up(at(el, 200, 300));
    expect(reply).not.toHaveBeenCalled();
  });
});
