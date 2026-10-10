// Search within the open conversation: the box, what it found, and whether the
// search ran at all. A failed search says so, rather than "no matches". On a
// phone the shell's own box is hidden while a thread is open, so this is the
// one there.

import { type Signal, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { Subject, catchError, of, switchMap } from 'rxjs';

import { MessagesApi } from './messages-api';
import { Origin, SearchHit } from './models';

export class ThreadSearch {
  readonly open = signal(false);
  readonly query = signal('');
  readonly hits = signal<SearchHit[] | null>(null);
  readonly busy = signal(false);
  /** A failed search, distinct from no hits. */
  readonly failed = signal(false);
  private readonly asked = new Subject<string>();

  /** Built with the thread, which is the injection context `takeUntilDestroyed` needs. */
  constructor(api: MessagesApi, origin: Signal<Origin | undefined>, id: Signal<string | undefined>) {
    // `switchMap`, so a slow answer cannot land after a newer one.
    this.asked
      .pipe(
        switchMap((q) => {
          const o = origin();
          const i = id();
          // Offered only on a routed conversation; never widened to global.
          if (o == null || i == null) return of<SearchHit[]>([]);
          return api.search(q, { origin: o, id: i }).pipe(
            catchError(() => {
              this.failed.set(true);
              return of<SearchHit[]>([]);
            }),
          );
        }),
        takeUntilDestroyed(),
      )
      .subscribe((hits) => {
        this.hits.set(hits);
        this.busy.set(false);
      });
  }

  /** Open or close the box, clearing what it found. */
  toggle(): void {
    const open = !this.open();
    this.open.set(open);
    if (!open) {
      this.query.set('');
      this.hits.set(null);
      this.failed.set(false);
    }
  }

  run(): void {
    const q = this.query().trim();
    if (!q) {
      this.hits.set(null);
      return;
    }
    this.busy.set(true);
    this.failed.set(false);
    this.asked.next(q);
  }
}
