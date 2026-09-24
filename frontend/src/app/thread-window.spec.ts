import { signal } from '@angular/core';
import { describe, expect, it } from 'vitest';

import { Message } from './models';
import { MAX_RENDERED, ThreadWindow } from './thread-window';

/** The bookkeeping half of the engine: the cap, window arithmetic, and
 *  reveal/collapse order. jsdom has no layout, so the measuring half is in
 *  `e2e/thread-scroll.spec.ts`. */

function msgs(n: number): Message[] {
  return Array.from({ length: n }, (_, i) => ({
    id: String(i),
    ts: 1_000_000 + i,
    sender: 's',
    is_outgoing: false,
    kind: 'message' as const,
    body: 'b',
    deleted: false,
    edited: false,
    reactions: [],
    attachments: [], link_images: [], link_offers: [], edits: [], reply_to: null, delivery: null, entities: [], album: null, previews: [],
  }));
}

function harness(n: number) {
  const host = document.createElement('div');
  const container = document.createElement('div');
  host.appendChild(container);
  const messages = signal(msgs(n));
  const win = new ThreadWindow(host, () => container, messages, () => undefined);
  return { win, messages, host, container };
}

describe('ThreadWindow', () => {
  it('renders everything while nothing is collapsed', () => {
    const { win } = harness(3);
    expect(win.renderCount()).toBe(3);
    expect(win.rendered().map((m) => m.id)).toEqual(['0', '1', '2']);
    expect(win.topSpacer()).toBe(0);
    expect(win.bottomSpacer()).toBe(0);
  });

  it('caps the DOM by collapsing the end away from the viewport', () => {
    const { win } = harness(1000);
    // 'bottom': it grew at the top, so the newest end is off-screen.
    win.enforceMax('bottom');
    expect(win.renderCount()).toBe(MAX_RENDERED);
    // The end kept is the one the reader is looking at.
    expect(win.rendered()[0].id).toBe('0');
    expect(win.rendered().at(-1)?.id).toBe(String(MAX_RENDERED - 1));
  });

  it('caps from the other end when the growth was at the bottom', () => {
    const { win } = harness(1000);
    win.enforceMax('top');
    expect(win.renderCount()).toBe(MAX_RENDERED);
    expect(win.rendered().at(-1)?.id).toBe('999');
    expect(win.rendered()[0].id).toBe(String(1000 - MAX_RENDERED));
  });

  it('leaves the window whole when nothing is off-screen to collapse', () => {
    // A viewport taller than the cap: nothing is off-screen, so the window stays
    // as it is. (Pins the outcome; the `break` itself is not observable here.)
    const { win } = harness(1000);
    win.trimToWindow();
    expect(win.renderCount()).toBe(1000);
  });

  // ---- following the end of the conversation --------------------------------
  //
  // Whether a resize re-pins to the bottom; whether it lands there is geometry,
  // in `e2e/ui-pages.spec.ts`.

  it('re-pins to the end when the container resizes under a reader who was at it', () => {
    const { win } = harness(10);
    win.scrollToBottom();
    expect(win.repinAfterResize()).toBe(true);
  });

  it('leaves a reader who is back in history where they are', () => {
    const { win } = harness(10);
    // A reader back in history stays there.
    win.scrollToTs(1_000_005);
    expect(win.repinAfterResize()).toBe(false);
  });

  it('takes the first scroll event for where the reader is, not for a resize', () => {
    // The open's scroll to the end and a reader's scroll back in the same frame
    // arrive as one event: the first `noteScroll`, with no height recorded yet.
    const { win, host } = harness(10);
    win.scrollToBottom();
    Object.defineProperty(host, 'clientHeight', { value: 800 });
    Object.defineProperty(host, 'scrollHeight', { value: 3000 });
    host.scrollTop = 0;
    win.noteScroll();
    expect(win.repinAfterResize()).toBe(false);
  });

  it('follows the end again for a fresh conversation', () => {
    const { win } = harness(10);
    win.scrollToTs(1_000_005);
    win.reset();
    expect(win.repinAfterResize()).toBe(true);
  });

  it('forgets what was collapsed when the conversation changes', () => {
    const { win } = harness(1000);
    win.enforceMax('bottom');
    expect(win.renderCount()).toBe(MAX_RENDERED);
    win.reset();
    expect(win.renderCount()).toBe(1000);
    expect(win.topSpacer()).toBe(0);
    expect(win.bottomSpacer()).toBe(0);
  });
});
