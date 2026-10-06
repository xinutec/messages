// A conversation's round picture: its initials on a hue of its own, and a badge
// saying which service it is on.

import { Component, computed, input } from '@angular/core';

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

  protected readonly hue = computed(() => hue(this.origin(), this.id()));
  protected readonly initials = computed(() => initials(this.name()));
  protected readonly service = computed(() => ORIGIN_LABELS[this.origin()]);
}
