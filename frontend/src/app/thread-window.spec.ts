import { signal } from '@angular/core';
import { describe, expect, it } from 'vitest';

import { Message } from './models';
import { MAX_RENDERED, ThreadWindow } from './thread-window';

/** What these pin, and what they deliberately do not.
 *
 *  The engine does two separable things: it MEASURES the page, and it keeps
 *  BOOKKEEPING about which run of messages is rendered and how tall the spacers
 *  standing in for the rest should be. jsdom has no layout, so every rect here
 *  would be zero and any test of the measuring half would be a test of a fake.
 *  That half stays with `e2e/thread-scroll.spec.ts`, in a real browser.
 *
 *  The bookkeeping half needs no layout at all, and it is where the cap, the
 *  window arithmetic and the reveal/collapse ordering live. Before the engine
 *  came out of `thread.ts` none of it could be reached without standing up a
 *  component, a router and an API; that was the argument for moving it, so this
 *  file is the argument being cashed. */

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
    attachments: [],
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
    // 'bottom' = grew at the top, so the newest end is the one off-screen.
    win.enforceMax('bottom');
    expect(win.renderCount()).toBe(MAX_RENDERED);
    // ⚠ The KEPT end is the one the user is looking at. Collapsing the wrong end
    // would leave the cap satisfied and the viewport empty, which is the failure
    // this direction exists to avoid.
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
    // A viewport taller than the cap. Nothing is rendered into this container,
    // so no message counts as off-screen — the same shape as that case, and the
    // window must come back untouched rather than collapsed blindly to the cap.
    //
    // ⚠ This pins the OUTCOME, not the `break` that produces it: deleting that
    // break leaves this green (measured), because the loop's guard then spins to
    // the same answer. Covering the mechanism would mean counting iterations,
    // which is a test of how it is written rather than what it does.
    const { win } = harness(1000);
    win.trimToWindow();
    expect(win.renderCount()).toBe(1000);
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
