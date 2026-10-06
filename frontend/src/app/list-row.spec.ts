import { describe, expect, it } from 'vitest';

import { hue, initials, preview, whenFormat } from './list-row';
import { Conversation, LastMessage } from './models';

const conv = (kind: Conversation['kind'], last: LastMessage | null): Conversation => ({
  origin: 'signal', id: 'x', name: 'X', kind, network: null, message_count: 1, last_ts: 1, last, unread: 0,
});
const msg = (over: Partial<LastMessage>): LastMessage => ({
  sender: 'Alice', is_outgoing: false, deleted: false, text: 'hi', ...over,
});

describe('initials', () => {
  it('takes the first two words', () => {
    expect(initials('Kansloos met een missie')).toBe('KM');
    expect(initials('alice')).toBe('A');
  });
  it('looks past a channel sigil', () => expect(initials('#nixos')).toBe('N'));
  it('keeps an emoji whole', () => expect(initials('🧠 notes')).toBe('🧠N'));
  it('never draws nothing', () => expect(initials('#')).toBe('?'));
});

describe('hue', () => {
  it('is stable for a conversation and differs between them', () => {
    const a = conv('dm', null);
    expect(hue(a)).toBe(hue({ ...a }));
    expect(hue(a)).not.toBe(hue({ ...a, id: 'y' }));
  });
});

describe('whenFormat', () => {
  // Local times, so the runner's time zone cannot move a day boundary.
  const now = new Date(2026, 9, 6, 14, 0);
  const at = (y: number, m: number, d: number, h = 12) => new Date(y, m, d, h).getTime();
  it('shows the time today', () => expect(whenFormat(at(2026, 9, 6, 0), now)).toBe('HH:mm'));
  it('shows the weekday this week', () => {
    expect(whenFormat(at(2026, 9, 5), now)).toBe('EEE');
    expect(whenFormat(at(2026, 8, 30), now)).toBe('EEE');
  });
  it('shows the day this year', () => expect(whenFormat(at(2026, 8, 29), now)).toBe('d MMM'));
  it('shows the year before that', () => expect(whenFormat(at(2025, 11, 31), now)).toBe('d MMM y'));
});

describe('preview', () => {
  it('names the sender in a group, not in a DM', () => {
    expect(preview(conv('group', msg({})))?.who).toBe('Alice');
    expect(preview(conv('dm', msg({})))?.who).toBeNull();
  });
  it('says You for my own message', () =>
    expect(preview(conv('dm', msg({ is_outgoing: true })))?.who).toBe('You'));
  it('says so when there are no words', () => {
    expect(preview(conv('dm', msg({ deleted: true, text: null })))).toEqual({
      who: null, text: 'Message deleted', quiet: true,
    });
    expect(preview(conv('dm', msg({ text: null })))?.text).toBe('No text');
  });
  it('has nothing for an empty conversation', () => expect(preview(conv('dm', null))).toBeNull());
});
