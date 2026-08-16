import { Component, computed, inject, signal } from '@angular/core';
import { DatePipe } from '@angular/common';
import { takeUntilDestroyed, toSignal } from '@angular/core/rxjs-interop';
import { ActivatedRoute, NavigationEnd, Router, RouterOutlet } from '@angular/router';
import { FormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatListModule } from '@angular/material/list';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatToolbarModule } from '@angular/material/toolbar';

import { Subject, catchError, filter, fromEvent, of, switchMap } from 'rxjs';

import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
import { Telemetry } from './telemetry';
import { Conversation, Origin, SearchHit } from './models';

@Component({
  selector: 'app-root',
  templateUrl: './app.html',
  styleUrl: './app.scss',
  imports: [
    RouterOutlet,
    DatePipe,
    FormsModule,
    MatToolbarModule,
    MatButtonModule,
    MatIconModule,
    MatListModule,
    MatFormFieldModule,
    MatInputModule,
    MatProgressBarModule,
  ],
})
export class App {
  private api = inject(MessagesApi);
  private store = inject(MessagesStore);
  private router = inject(Router);
  private route = inject(ActivatedRoute);
  // Instrumented from the shell alone: a trace each screen had to remember to
  // join would have holes in exactly the screens nobody thought about.
  private telemetry = inject(Telemetry);

  readonly me = this.store.me;
  readonly loading = this.store.loading;
  readonly conversations = this.store.conversations;

  /** One label per origin, and the order the filter buttons appear in.
   *
   *  A `Record` rather than a chain of ternaries. The template read
   *  `origin === 'signal' ? 'Signal' : 'Google Chat'`, which does not fail when
   *  a third origin arrives — it labels it as the second one. This is a type
   *  error the day a fourth does. */
  readonly originLabels: Record<Origin, string> = {
    signal: 'Signal',
    gchat: 'Google Chat',
    irc: 'IRC',
  };
  readonly origins: readonly Origin[] = ['signal', 'gchat', 'irc'];

  // ?origin filters the list — view-state on the route, like health's ?date.
  private params = toSignal(this.route.queryParamMap);
  readonly originFilter = computed<Origin | 'all'>(() => {
    const o = this.params()?.get('origin');
    // Matched against the list rather than compared to literals, so an origin
    // added above is filterable without touching this.
    return this.origins.find((known) => known === o) ?? 'all';
  });

  // The open conversation is whatever the child route resolved to. Deriving it
  // from the router (recomputed each navigation) lets the list highlight it and
  // the mobile single-pane switch — without the shell owning that state.
  private navEnd = toSignal(this.router.events.pipe(filter((e) => e instanceof NavigationEnd)));
  readonly active = computed<{ origin: string; id: string } | null>(() => {
    this.navEnd();
    const pm = this.leaf().snapshot.paramMap; // typed get() → string | null
    const origin = pm.get('origin');
    const id = pm.get('id');
    return origin != null && id != null ? { origin, id } : null;
  });

  // Search overlays the list; it's transient UI, not URL state. Queries run
  // through a Subject + switchMap so a slow response can't land after a newer
  // one (the stale request is cancelled).
  readonly query = signal('');
  readonly results = signal<SearchHit[] | null>(null);
  readonly searching = signal(false);
  /**
   * The search did not run. ⚠ **A distinct state from "no hits", because the
   * empty list is a CLAIM** — the template answers it with "No matches.", which
   * says the messages are not there. A 500 or a dropped connection swallowed
   * into that claim tells a reader their conversation does not exist.
   */
  readonly searchFailed = signal(false);
  private search$ = new Subject<string>();

  readonly visibleConversations = computed(() => {
    const f = this.originFilter();
    const list = this.conversations();
    return f === 'all' ? list : list.filter((c) => c.origin === f);
  });

  constructor() {
    this.store.init();
    this.telemetry.init();

    // Re-read the list on the way back to it. Closing a conversation is the
    // moment its unread count and last-message time are about to be read, and it
    // is a deliberate act rather than a tick.
    let wasOpen = false;
    this.router.events
      .pipe(
        filter((e) => e instanceof NavigationEnd),
        takeUntilDestroyed(),
      )
      .subscribe(() => {
        const open = this.active() != null;
        if (wasOpen && !open) this.store.refresh();
        wasOpen = open;
      });

    // ⚠ **The same moment, by a route the router never sees.** Coming back from
    // the Android launcher is a return to the list, but nothing navigates — it is
    // already mounted and keeps showing what it last rendered. Measured on the
    // Pixel 9: two hours old, ten behind on one channel, one row out of order,
    // and nothing about it looking stale.
    //
    // Unconditional, deliberately: on a wide screen the list shares the window
    // with the open thread, so an "only when the list is the visible screen" gate
    // would skip the one case where a stale list sits beside a thread that
    // refreshed itself.
    fromEvent(document, 'visibilitychange')
      .pipe(takeUntilDestroyed())
      .subscribe(() => {
        if (document.visibilityState === 'visible') this.store.refresh();
      });

    this.search$
      .pipe(
        switchMap((q) =>
          this.api.search(q).pipe(
            catchError(() => {
              this.searchFailed.set(true);
              return of<SearchHit[]>([]);
            }),
          ),
        ),
        takeUntilDestroyed(),
      )
      .subscribe((hits) => {
        this.results.set(hits);
        this.searching.set(false);
      });
  }

  private leaf(): ActivatedRoute {
    let r = this.route.root;
    while (r.firstChild) r = r.firstChild;
    return r;
  }

  isActive(c: Conversation): boolean {
    const a = this.active();
    return a?.origin === c.origin && a?.id === c.id;
  }

  setFilter(f: Origin | 'all'): void {
    // Update ?origin on the current route (keep the open conversation, if any).
    void this.router.navigate([], {
      relativeTo: this.leaf(),
      queryParams: { origin: f === 'all' ? null : f },
      queryParamsHandling: 'merge',
    });
  }

  /** Open a conversation = route to it; keep the origin filter, reset ?from so a
   *  freshly-opened conversation starts at the most recent page. */
  open(c: Conversation): void {
    void this.router.navigate(['/conversation', c.origin, c.id], {
      queryParams: { from: null },
      queryParamsHandling: 'merge',
    });
  }

  runSearch(): void {
    const q = this.query().trim();
    if (!q) {
      this.results.set(null);
      return;
    }
    this.searching.set(true);
    this.searchFailed.set(false);
    this.search$.next(q);
  }

  clearSearch(): void {
    this.query.set('');
    this.results.set(null);
    this.searchFailed.set(false);
  }


  /** Open the conversation a search hit belongs to (looked up from the list). */
  openHit(h: SearchHit): void {
    const c = this.store.find(h.origin, h.conversation_id);
    if (c) this.open(c);
  }

  title(c: Conversation): string {
    return this.store.title(c);
  }

  signOut(): void {
    this.api.logout().subscribe(() => (window.location.href = '/'));
  }
}
