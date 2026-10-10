// A conversation's round picture: its own where the origin keeps one, else its
// initials on a hue of its own; and a badge saying which service it is on.

import { Component, computed, input, linkedSignal } from '@angular/core';

import { hue, initials, ORIGIN_LABELS } from './list-row';
import { Origin } from './models';

@Component({
  selector: 'app-avatar',
  templateUrl: './avatar.html',
  styleUrl: './avatar.scss',
  host: { '[style.--hue]': 'hue()' },
})
export class Avatar {
  readonly origin = input.required<Origin>();
  readonly id = input.required<string>();
  readonly name = input.required<string>();
  /** The list's `avatar`: the picture's version, or null for none. */
  readonly version = input<number | null>(null);
  /** The service badge; the bar leaves it off, where the circle is too small to
   *  share. */
  readonly badge = input(true);

  protected readonly hue = computed(() => hue(this.origin(), this.id()));
  protected readonly initials = computed(() => initials(this.name()));
  protected readonly service = computed(() => ORIGIN_LABELS[this.origin()]);
  /** Where the picture is; the version makes a new one a new URL. */
  protected readonly picture = computed(() => {
    const v = this.version();
    return v === null
      ? null
      : `/api/conversations/${this.origin()}/${encodeURIComponent(this.id())}/avatar?v=${v}`;
  });
  /** A picture that would not load shows the initials; a new one tries again. */
  protected readonly failed = linkedSignal({ source: this.picture, computation: () => false });
}
