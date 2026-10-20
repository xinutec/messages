import { convertToParamMap } from '@angular/router';
import { describe, expect, it } from 'vitest';

import { ReaderPlace } from './reader-place';

const params = (p: Record<string, string> = {}) => convertToParamMap(p);

describe('ReaderPlace', () => {
  it('marks the message it was put on until the reader scrolls', () => {
    const place = new ReaderPlace(params());
    place.landOn('h', true);
    expect(place.landedId()).toBe('h');
    expect(place.floating()).toBe(true);
    place.readerScrolled();
    expect(place.landedId()).toBeNull();
    expect(place.hasScrolled).toBe(true);
  });

  it('a new landing puts what the reader scrolled behind it', () => {
    const place = new ReaderPlace(params());
    place.readerScrolled();
    place.landOn('r', false);
    expect(place.hasScrolled).toBe(false);
    expect(place.landedId()).toBe('r');
  });

  it('starts a fresh conversation at its end, nothing marked', () => {
    const place = new ReaderPlace(params());
    place.landOn('h', true);
    place.readerScrolled();
    place.reset();
    expect(place.landedId()).toBeNull();
    expect(place.floating()).toBe(false);
    expect(place.hasScrolled).toBe(false);
  });

  describe('the address', () => {
    it('says nothing new when it already names the place', () => {
      const place = new ReaderPlace(params());
      expect(place.address(params(), null)).toBeNull();
      expect(place.address(params({ from: '1000' }), 1000)).toBeNull();
    });

    it('writes where the view is', () => {
      const place = new ReaderPlace(params());
      expect(place.address(params(), 1000)).toEqual({ from: '1000' });
      expect(place.address(params({ from: '1000' }), null)).toEqual({ from: null });
    });

    it('keeps the landing until the reader scrolls, then drops it', () => {
      const place = new ReaderPlace(params({ at: '5000_9' }));
      place.landOn('h', true);
      expect(place.address(params({ at: '5000_9' }), 5000)).toEqual({ from: '5000' });
      place.readerScrolled();
      expect(place.address(params({ at: '5000_9', from: '5000' }), 5000)).toEqual({
        from: '5000',
        at: null,
        on: null,
      });
    });
  });

  describe('a change of address', () => {
    it('is a new landing when it names another hit or day', () => {
      const place = new ReaderPlace(params({ at: 'a' }));
      expect(place.isNewLanding(params({ at: 'b' }))).toBe(true);
      expect(place.isNewLanding(params({ at: 'b', on: '7' }))).toBe(true);
    });

    it('is not one when it names the same place, or drops the landing', () => {
      const place = new ReaderPlace(params({ at: 'a' }));
      expect(place.isNewLanding(params({ at: 'a', from: '1' }))).toBe(false);
      expect(place.isNewLanding(params({ from: '1' }))).toBe(false);
      // Back to the same hit after it was dropped is a landing again.
      expect(place.isNewLanding(params({ at: 'a' }))).toBe(true);
    });
  });
});
