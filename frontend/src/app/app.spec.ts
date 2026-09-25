import { ComponentFixture, TestBed } from '@angular/core/testing';
import { provideZonelessChangeDetection } from '@angular/core';
import { Router, provideRouter } from '@angular/router';
import { provideServiceWorker } from '@angular/service-worker';
import { of, throwError } from 'rxjs';
import { describe, expect, it, vi } from 'vitest';

import { App } from './app';
import { MessagesApi } from './messages-api';
import { Conversation, Me, Message, MessagesPage, SearchHit } from './models';
import { testMessage } from './test-message';

const msg = (id: string, ts: number): Message => testMessage({ id, ts });

const CONVS: Conversation[] = [
  { origin: 'signal', id: 'dm:a', name: 'Alice', kind: 'dm', network: null, message_count: 5, last_ts: 200 },
  { origin: 'gchat', id: 'gc1', name: 'Bob', kind: 'dm', network: null, message_count: 3, last_ts: 300 },
];

/** One IRC target on two networks. */
const TWO_NETWORKS: Conversation[] = [
  { origin: 'irc', id: '8', name: 's_20', kind: 'dm', network: 'xinutec', message_count: 14446, last_ts: 200 },
  { origin: 'irc', id: '9', name: 's_20', kind: 'dm', network: 'euirc', message_count: 8071, last_ts: 100 },
  { origin: 'gchat', id: 'gc1', name: 'Bob', kind: 'dm', network: null, message_count: 3, last_ts: 300 },
];

const ME: Me = { user_id: 'u1', display_name: 'Test User' };

function makeApi(over: { search?: ReturnType<typeof vi.fn>; conversations?: ReturnType<typeof vi.fn> } = {}) {
  return {
    me: vi.fn(() => of(ME)),
    conversations: over.conversations ?? vi.fn(() => of(CONVS)),
    messages: vi.fn(() => of({ messages: [msg('1', 100)], has_more: false, next_cursor: null, prev_cursor: null } as MessagesPage)),
    search: over.search ?? vi.fn(() => of([] as SearchHit[])),
    logout: vi.fn(() => of({})),
  } as unknown as MessagesApi;
}

/** The search list, rendered: where the retraction rule is applied. */
function render(api: MessagesApi): ComponentFixture<App> {
  TestBed.configureTestingModule({
    providers: [
      provideZonelessChangeDetection(),
      provideRouter([]),
      // The real SwUpdate, disabled, as Angular documents for tests; `App`
      // starts the updater in its constructor.
      provideServiceWorker('ngsw-worker.js', { enabled: false }),
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
      // As above.
      provideServiceWorker('ngsw-worker.js', { enabled: false }),
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

  // These navigate; the URL-to-state wiring is in e2e/routing.spec.ts.
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
    // `from` cleared; the origin filter kept.
    expect(nav).toHaveBeenCalledWith(['/conversation', 'signal', 'dm:a'], expect.objectContaining({ queryParams: { from: null }, queryParamsHandling: 'merge' }));
  });

  it('openHit routes to the conversation a search hit belongs to', () => {
    const { app, router } = setup(makeApi());
    const nav = vi.spyOn(router, 'navigate').mockResolvedValue(true);
    app.openHit({ origin: 'gchat', conversation_id: 'gc1', conversation_name: 'Bob', ts: 1, sender: 's', snippet: 'x', deleted: false, cursor: '1_1' });
    expect(nav).toHaveBeenCalledWith(['/conversation', 'gchat', 'gc1'], expect.objectContaining({ queryParams: { at: '1_1', from: null } }));
  });

  /** A hit opens from its own origin and id, before the list has loaded. */
  it('opens a hit even when the conversation list never loaded', () => {
    const conversations = vi.fn(() => throwError(() => new Error('offline')));
    const { app, router } = setup(makeApi({ conversations }));
    expect(app.conversations()).toEqual([]); // the precondition, not an assumption
    const nav = vi.spyOn(router, 'navigate').mockResolvedValue(true);
    app.openHit({ origin: 'irc', conversation_id: '7', conversation_name: '#chan', ts: 1, sender: 's', snippet: 'x', deleted: false, cursor: '1_1' });
    expect(nav).toHaveBeenCalledWith(['/conversation', 'irc', '7'], expect.objectContaining({ queryParams: { at: '1_1', from: null } }));
  });

  /** Search names a conversation as the list does, whitespace-only names
   *  included. */
  it('names a hit the way the conversation list names it', () => {
    const { app } = setup(makeApi());
    const hit = { origin: 'signal' as const, conversation_id: 'dm:a', conversation_name: null, ts: 1, sender: 's', snippet: 'x', deleted: false, cursor: '1_1' };
    // Named from the list, though the hit carries no name.
    expect(app.hitTitle(hit)).toBe('Alice');
    expect(app.hitTitle({ ...hit, conversation_id: 'nope' })).toBe('Conversation');
    expect(app.hitTitle({ ...hit, conversation_id: 'nope', conversation_name: '   ' })).toBe('Conversation');
  });

  /** Search rows show the network, as the list does. */
  it('says which archive and which network a hit came from', () => {
    const { app } = setup(makeApi({ conversations: vi.fn(() => of(TWO_NETWORKS)) }));
    const hit = { conversation_name: 's_20', ts: 1, sender: 's', snippet: 'x', deleted: false, cursor: '1_1' };
    expect(app.hitOrigin({ ...hit, origin: 'irc', conversation_id: '8' })).toBe('IRC xinutec');
    expect(app.hitOrigin({ ...hit, origin: 'irc', conversation_id: '9' })).toBe('IRC euirc');
    // Signal and Google Chat have no network.
    expect(app.hitOrigin({ ...hit, origin: 'gchat', conversation_id: 'gc1' })).toBe('Google Chat');
    // Unknown to the list: still the archive.
    expect(app.hitOrigin({ ...hit, origin: 'irc', conversation_id: 'nope' })).toBe('IRC');
  });

  it('runs a search and clears it', () => {
    const hit: SearchHit = { origin: 'signal', conversation_id: 'dm:a', conversation_name: 'Alice', ts: 1, sender: 's', snippet: 'hi', deleted: false, cursor: '1_1' };
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

  /** A retracted hit's snippet reaches the browser, and the template must not
   *  show it. */
  it('does not print what a retracted message said', async () => {
    const hit: SearchHit = { origin: 'signal', conversation_id: 'dm:a', conversation_name: 'Alice', ts: 5, sender: 'alice', snippet: 'withdrawn text', deleted: true, cursor: '1_1' };
    const fixture = render(makeApi({ search: vi.fn(() => of([hit])) }));
    fixture.componentInstance.query.set('withdrawn');
    fixture.componentInstance.runSearch();
    await fixture.whenStable();
    fixture.detectChanges();

    const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
    expect(text).not.toContain('withdrawn text');
    // Still listed: whose and when.
    expect(text).toContain('(deleted)');
    expect(text).toContain('alice');
    expect(text).toContain('Alice');
  });

  /** An ordinary snippet is shown, or hiding every snippet would pass above. */
  it('prints an ordinary hit in full', async () => {
    const hit: SearchHit = { origin: 'signal', conversation_id: 'dm:a', conversation_name: 'Alice', ts: 5, sender: 'alice', snippet: 'still here', deleted: false, cursor: '1_1' };
    const fixture = render(makeApi({ search: vi.fn(() => of([hit])) }));
    fixture.componentInstance.query.set('still');
    fixture.componentInstance.runSearch();
    await fixture.whenStable();
    fixture.detectChanges();

    const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
    expect(text).toContain('still here');
    expect(text).not.toContain('(deleted)');
  });

  /** Rows are tracked by index: an IRC hit's ts has second resolution, so two
   *  lines in one second would collide (NG0955), and a hit carries no unique id.
   *  The array is replaced wholesale on every search. */
  it('renders two hits from the same channel in the same second', async () => {
    const at = Date.UTC(2026, 0, 2, 9, 14, 0);
    const both: SearchHit[] = [
      { origin: 'irc', conversation_id: '7', conversation_name: '#chan', ts: at, sender: 'a', snippet: 'first line', deleted: false, cursor: '1_1' },
      { origin: 'irc', conversation_id: '7', conversation_name: '#chan', ts: at, sender: 'b', snippet: 'second line', deleted: false, cursor: '1_1' },
    ];
    const warnings: string[] = [];
    const spy = vi.spyOn(console, 'warn').mockImplementation((...a) => { warnings.push(a.map(String).join(' ')); });
    const err = vi.spyOn(console, 'error').mockImplementation((...a) => { warnings.push(a.map(String).join(' ')); });
    try {
      const fixture = render(makeApi({ search: vi.fn(() => of(both)) }));
      fixture.componentInstance.query.set('line');
      fixture.componentInstance.runSearch();
      await fixture.whenStable();
      fixture.detectChanges();

      const text = (fixture.nativeElement as HTMLElement).textContent ?? '';
      expect(text).toContain('first line');
      expect(text).toContain('second line');
      expect(warnings.join(' ')).not.toContain('NG0955');
    } finally {
      spy.mockRestore();
      err.mockRestore();
    }
  });

  it('title falls back when a conversation is unnamed', () => {
    const { app } = setup(makeApi());
    expect(app.title({ ...CONVS[0], name: null })).toBe('Direct message');
    expect(app.title({ ...CONVS[0], name: '', kind: 'group' })).toBe('Group');
    expect(app.title(CONVS[0])).toBe('Alice');
  });

  /** An Android resume fires `visibilitychange`, not the router. */
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

  /** The listener is removed with the component. */
  it('stops listening once the component is destroyed', () => {
    const conversations = vi.fn(() => of(CONVS));
    setup(makeApi({ conversations }));
    TestBed.resetTestingModule(); // destroys the injector the component was made in
    document.dispatchEvent(new Event('visibilitychange'));
    expect(conversations).toHaveBeenCalledTimes(1);
  });

  /** Every origin has its own label. */
  it('labels every origin as itself, with none borrowing another name', () => {
    const { app } = setup(makeApi());
    const labels = app.origins.map((o) => app.originLabels[o]);
    expect(labels).toEqual(['Signal', 'Google Chat', 'IRC', 'Telegram']);
    expect(new Set(labels).size).toBe(app.origins.length);
    for (const label of labels) expect(label).not.toBe('');
  });

  /** Every filterable origin has a label. */
  it('has a label for each filter button and no orphans', () => {
    const { app } = setup(makeApi());
    expect([...app.origins].sort()).toEqual(Object.keys(app.originLabels).sort());
  });
});
