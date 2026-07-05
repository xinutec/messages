import { ComponentRef, provideZonelessChangeDetection } from '@angular/core';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { Router, provideRouter } from '@angular/router';
import { of } from 'rxjs';
import { describe, expect, it, vi } from 'vitest';

import { Thread } from './thread';
import { MessagesApi } from './messages-api';
import { Message, MessagesPage } from './models';

function msg(id: string, ts: number): Message {
  return { id, ts, sender: 's', is_outgoing: false, body: 'b', deleted: false, edited: false, reactions: [], attachments: [] };
}

function makeApi() {
  return {
    me: vi.fn(() => of({ user_id: 'u1', display_name: 'Test User' })),
    conversations: vi.fn(() => of([])),
    messages: vi.fn(() => of({ messages: [msg('1', 100)], has_more: false, next_cursor: null } as MessagesPage)),
    search: vi.fn(() => of([])),
    logout: vi.fn(() => of({})),
  } as unknown as MessagesApi;
}

function setup(): { thread: Thread; ref: ComponentRef<Thread>; fixture: ComponentFixture<Thread>; router: Router } {
  TestBed.configureTestingModule({
    providers: [
      provideZonelessChangeDetection(),
      provideRouter([]),
      { provide: MessagesApi, useValue: makeApi() },
    ],
  });
  // createComponent (not `new Thread()`): the component injects ElementRef, which
  // only exists for a real component instance.
  const fixture = TestBed.createComponent(Thread);
  return { thread: fixture.componentInstance, ref: fixture.componentRef, fixture, router: TestBed.inject(Router) };
}

describe('Thread', () => {
  it('dayGroups buckets consecutive rendered messages by calendar day', () => {
    const { thread } = setup();
    const d1 = new Date(2026, 5, 1, 9, 0, 0).getTime();
    const d1b = new Date(2026, 5, 1, 18, 0, 0).getTime();
    const d2 = new Date(2026, 5, 2, 9, 0, 0).getTime();
    // With nothing collapsed, the rendered window is the whole retained list.
    thread.messages.set([msg('a', d1), msg('b', d1b), msg('c', d2)]);
    const groups = thread.dayGroups();
    expect(groups.length).toBe(2);
    expect(groups[0].items.map((m) => m.id)).toEqual(['a', 'b']);
    expect(groups[1].items.map((m) => m.id)).toEqual(['c']);
  });

  it('rendered window equals retained messages when nothing is collapsed', () => {
    const { thread } = setup();
    thread.messages.set([msg('a', 1), msg('b', 2), msg('c', 3)]);
    expect(thread.renderCount()).toBe(3);
    expect(thread.rendered().map((m) => m.id)).toEqual(['a', 'b', 'c']);
    expect(thread.topSpacer()).toBe(0);
    expect(thread.bottomSpacer()).toBe(0);
  });

  it('back returns to the list route, dropping the paged depth', () => {
    const { thread, router } = setup();
    const nav = vi.spyOn(router, 'navigate').mockResolvedValue(true);
    thread.back();
    // origin filter preserved (merge); from cleared.
    expect(nav).toHaveBeenCalledWith(['/'], expect.objectContaining({ queryParams: { from: null }, queryParamsHandling: 'merge' }));
  });
});
