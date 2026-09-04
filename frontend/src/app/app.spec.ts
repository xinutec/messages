import { ComponentFixture, TestBed } from '@angular/core/testing';
import { provideZonelessChangeDetection } from '@angular/core';
import { Router, provideRouter } from '@angular/router';
import { of } from 'rxjs';
import { describe, expect, it, vi } from 'vitest';

import { App } from './app';
import { MessagesApi } from './messages-api';
import { Conversation, Me, Message, MessagesPage, SearchHit } from './models';

function msg(id: string, ts: number, extra: Partial<Message> = {}): Message {
  return {
    id,
    ts,
    sender: 's',
    is_outgoing: false,
    kind: 'message',
    body: 'b',
    deleted: false,
    edited: false,
    reactions: [],
    attachments: [],
    ...extra,
  };
}

const CONVS: Conversation[] = [
  { origin: 'signal', id: 'dm:a', name: 'Alice', kind: 'dm', network: null, message_count: 5, last_ts: 200 },
  { origin: 'gchat', id: 'gc1', name: 'Bob', kind: 'dm', network: null, message_count: 3, last_ts: 300 },
];

const ME: Me = { user_id: 'u1', display_name: 'Test User' };

function makeApi(over: { search?: ReturnType<typeof vi.fn>; conversations?: ReturnType<typeof vi.fn> } = {}) {
  return {
    me: vi.fn(() => of(ME)),
    conversations: over.conversations ?? vi.fn(() => of(CONVS)),
    messages: vi.fn(() => of({ messages: [msg('1', 100)], has_more: false, next_cursor: null } as MessagesPage)),
    search: over.search ?? vi.fn(() => of([] as SearchHit[])),
    logout: vi.fn(() => of({})),
  } as unknown as MessagesApi;
}

/** The search list, actually rendered. The rest of this file drives the model;
 *  the two assertions below are about what reaches the screen, which is where
 *  the retraction rule is applied. */
function render(api: MessagesApi): ComponentFixture<App> {
  TestBed.configureTestingModule({
    providers: [
      provideZonelessChangeDetection(),
      provideRouter([]),
      { provide: MessagesApi, useValue: api },
    ],
  });
  const fixture = TestBed.createComponent(App);
  fixture.detectChanges();
  return fixture;
}

function setup(api: MessagesApi): { app: App; router: Router } {
  TestBed.configureTestingModule({
    providers: [
      provideZonelessChangeDetection(),
      provideRouter([]),
      { provide: MessagesApi, useValue: api },
    ],
  });
  const app = TestBed.runInInjectionContext(() => new App());
  return { app, router: TestBed.inject(Router) };
}

describe('App', () => {
  it('loads the user and conversations on init', () => {
    const { app } = setup(makeApi());
    expect(app.me()?.user_id).toBe('u1');
    expect(app.conversations().length).toBe(2);
  });

  it('shows all conversations by default (no origin filter in the URL)', () => {
    const { app } = setup(makeApi());
    expect(app.originFilter()).toBe('all');
    expect(app.visibleConversations().length).toBe(2);
  });

  // Navigation state lives in the URL: these actions navigate; the URL→state
  // wiring (filtering, opening, Back) is covered by e2e/routing.spec.ts.
  it('setFilter navigates with the origin query param', () => {
    const { app, router } = setup(makeApi());
    const nav = vi.spyOn(router, 'navigate').mockResolvedValue(true);
    app.setFilter('signal');
    expect(nav).toHaveBeenCalledWith([], expect.objectContaining({ queryParams: { origin: 'signal' }, queryParamsHandling: 'merge' }));
    app.setFilter('all');
    expect(nav).toHaveBeenLastCalledWith([], expect.objectContaining({ queryParams: { origin: null } }));
  });

  it('open routes to the conversation as a path', () => {
    const { app, router } = setup(makeApi());
    const nav = vi.spyOn(router, 'navigate').mockResolvedValue(true);
    app.open(CONVS[0]);
    // from: null resets paged-back depth; origin filter is preserved (merge).
    expect(nav).toHaveBeenCalledWith(['/conversation', 'signal', 'dm:a'], expect.objectContaining({ queryParams: { from: null }, queryParamsHandling: 'merge' }));
  });

  it('openHit routes to the conversation a search hit belongs to', () => {
    const { app, router } = setup(makeApi());
    const nav = vi.spyOn(router, 'navigate').mockResolvedValue(true);
    app.openHit({ origin: 'gchat', conversation_id: 'gc1', conversation_name: 'Bob', ts: 1, sender: 's', snippet: 'x', deleted: false });
    expect(nav).toHaveBeenCalledWith(['/conversation', 'gchat', 'gc1'], expect.objectContaining({ queryParams: { from: null } }));
  });

  it('runs a search and clears it', () => {
    const hit: SearchHit = { origin: 'signal', conversation_id: 'dm:a', conversation_name: 'Alice', ts: 1, sender: 's', snippet: 'hi', deleted: false };
    const search = vi.fn(() => of([hit]));
    const { app } = setup(makeApi({ search }));
    app.query.set('hi');
    app.runSearch();
    expect(app.results()).toEqual([hit]);
    app.clearSearch();
    expect(app.results()).toBeNull();
    expect(app.query()).toBe('');
  });

  it('ignores a blank search', () => {
    const search = vi.fn(() => of([] as SearchHit[]));
    const { app } = setup(makeApi({ search }));
    app.query.set('   ');
    app.runSearch();
    expect(search).not.toHaveBeenCalled();
    expect(app.results()).toBeNull();
  });

  /**
   * ⚠ **THE ASSERTION THAT MATTERS IS AN ABSENCE.** Search stopped filtering
   * `deleted = 0` on 2026-09-04 so that a retraction can be found; the text now
   * arrives in the browser and only the template keeps it off the screen. That
   * makes the list one careless edit away from printing what somebody withdrew,
   * and nothing about the model would look wrong — `results()` is *supposed* to
   * hold the words.
   *
   * jsdom can answer this one: it is text content, not layout. What it cannot
   * answer is whether the dimmed italic reads as retracted, which is why the
   * thread's affordance was settled by looking at the render instead.
   */
  it('does not print what a retracted message said', async () => {
    const hit: SearchHit = { origin: 'signal', conversation_id: 'dm:a', conversation_name: 'Alice', ts: 5, sender: 'alice', snippet: 'withdrawn text', deleted: true };
    const fixture = render(makeApi({ search: vi.fn(() => of([hit])) }));
    fixture.componentInstance.query.set('withdrawn');
    fixture.componentInstance.runSearch();
    await fixture.whenStable();
    fixture.detectChanges();

    const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
    expect(text).not.toContain('withdrawn text');
    // The hit is still a hit: whose and when, and that there is something here.
    expect(text).toContain('(deleted)');
    expect(text).toContain('alice');
    expect(text).toContain('Alice');
  });

  /** The other half. A gate that hid every snippet would satisfy the test above
   *  and leave search showing nothing at all. */
  it('prints an ordinary hit in full', async () => {
    const hit: SearchHit = { origin: 'signal', conversation_id: 'dm:a', conversation_name: 'Alice', ts: 5, sender: 'alice', snippet: 'still here', deleted: false };
    const fixture = render(makeApi({ search: vi.fn(() => of([hit])) }));
    fixture.componentInstance.query.set('still');
    fixture.componentInstance.runSearch();
    await fixture.whenStable();
    fixture.detectChanges();

    const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
    expect(text).toContain('still here');
    expect(text).not.toContain('(deleted)');
  });

  it('title falls back when a conversation is unnamed', () => {
    const { app } = setup(makeApi());
    expect(app.title({ ...CONVS[0], name: null })).toBe('Direct message');
    expect(app.title({ ...CONVS[0], name: '', kind: 'group' })).toBe('Group');
    expect(app.title(CONVS[0])).toBe('Alice');
  });

  /** The router never fires on an Android resume — the list is already mounted —
   *  so `visibilitychange` is the only signal that the user came back to look. */
  it('re-reads the list when the app returns to the foreground', () => {
    const conversations = vi.fn(() => of(CONVS));
    setup(makeApi({ conversations }));
    expect(conversations).toHaveBeenCalledTimes(1); // the initial load

    document.dispatchEvent(new Event('visibilitychange')); // jsdom: 'visible'
    expect(conversations).toHaveBeenCalledTimes(2);
  });

  it('does not re-read when the app goes to the background', () => {
    const conversations = vi.fn(() => of(CONVS));
    setup(makeApi({ conversations }));
    const hidden = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    try {
      document.dispatchEvent(new Event('visibilitychange'));
      expect(conversations).toHaveBeenCalledTimes(1);
    } finally {
      hidden.mockRestore();
    }
  });

  /** A listener on `document` outlives the component unless something removes
   *  it. Without `takeUntilDestroyed` each teardown would leave one behind,
   *  firing a query for a screen that is gone — invisible in a browser, and a
   *  test that only checked the refresh happens would still pass. */
  it('stops listening once the component is destroyed', () => {
    const conversations = vi.fn(() => of(CONVS));
    setup(makeApi({ conversations }));
    TestBed.resetTestingModule(); // destroys the injector the component was made in
    document.dispatchEvent(new Event('visibilitychange'));
    expect(conversations).toHaveBeenCalledTimes(1);
  });

  /** Every origin names itself. The template used to read
   *  `origin === 'signal' ? 'Signal' : 'Google Chat'`, which does not *fail* on a
   *  third origin — it labels IRC as Google Chat, on the conversation list, for
   *  every row. A missing label would be a blank; a wrong one is a lie. */
  it('labels every origin as itself, with none borrowing another name', () => {
    const { app } = setup(makeApi());
    const labels = app.origins.map((o) => app.originLabels[o]);
    expect(labels).toEqual(['Signal', 'Google Chat', 'IRC']);
    expect(new Set(labels).size).toBe(app.origins.length);
    for (const label of labels) expect(label).not.toBe('');
  });

  /** The filter list and the label table are two statements of one fact, so a
   *  new origin added to only one of them is caught here rather than by a blank
   *  button in the toolbar. */
  it('has a label for each filter button and no orphans', () => {
    const { app } = setup(makeApi());
    expect([...app.origins].sort()).toEqual(Object.keys(app.originLabels).sort());
  });
});
