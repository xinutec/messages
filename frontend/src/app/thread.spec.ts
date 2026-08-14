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
    send: vi.fn(() => of({ sent: true, error: null, archived: true })),
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
  it('offers a composer for IRC only — the other origins have no live client', () => {
    const { thread, ref, fixture } = setup();
    ref.setInput('origin', 'signal');
    ref.setInput('id', 'dm:a');
    fixture.detectChanges();
    expect(thread.canSend()).toBe(false);

    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();
    expect(thread.canSend()).toBe(true);
  });

  it('keeps the draft when the send is refused, and clears it only once it went', async () => {
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as { send: ReturnType<typeof vi.fn> };
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();

    // The far side refuses — most often because the recipient is not on the
    // allow-list held on the irssi host.
    api.send.mockReturnValueOnce(of({ sent: false, error: 'refused: not on the allow-list', archived: false }));
    thread.draft.set('please keep me');
    await thread.send();
    expect(thread.draft()).toBe('please keep me');
    expect(thread.sendError()).toContain('allow-list');

    // ⚠ The box is emptied only on a real send. Clearing on failure loses what
    // was typed at the exact moment the person has to type it again.
    api.send.mockReturnValueOnce(of({ sent: true, error: null, archived: true }));
    await thread.send();
    expect(thread.draft()).toBe('');
  });

  it('says so when a message went but is not in the archive yet', async () => {
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as { send: ReturnType<typeof vi.fn> };
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();

    // Sent, but irssi could not find the echo in its log — the hourly import
    // will bring it. Silence would show a conversation that appears not to
    // contain the message just sent.
    api.send.mockReturnValueOnce(of({ sent: true, error: null, archived: false }));
    thread.draft.set('gone, but not seen');
    await thread.send();
    expect(thread.draft()).toBe('');
    expect(thread.sendError()).toContain('after the next import');
  });

  it('does not send an empty or whitespace-only draft', async () => {
    const { thread, ref, fixture } = setup();
    const api = TestBed.inject(MessagesApi) as unknown as { send: ReturnType<typeof vi.fn> };
    ref.setInput('origin', 'irc');
    ref.setInput('id', '7');
    fixture.detectChanges();

    thread.draft.set('   ');
    await thread.send();
    expect(api.send).not.toHaveBeenCalled();
  });
});
