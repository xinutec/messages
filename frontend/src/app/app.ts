import { Component, computed, inject, signal } from '@angular/core';
import { AppSwUpdates } from './sw-updates';
import { BUILD_INFO } from './build-info';
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
  private swUpdates = inject(AppSwUpdates);

  /** Which build this is, from the bundle; see app.html. */
  protected readonly build = BUILD_INFO;
  protected readonly builtAt = new Date(BUILD_INFO.builtAt).toLocaleString();
  private store = inject(MessagesStore);
  private router = inject(Router);
  private route = inject(ActivatedRoute);
  // Instrumented once, in the shell, so no screen can forget to join.
  private telemetry = inject(Telemetry);

  readonly me = this.store.me;
  readonly loading = this.store.loading;
  readonly conversations = this.store.conversations;

  /** One label per origin, in filter-button order. A `Record`, so a new origin
   *  is a type error until labelled. */
  readonly originLabels: Record<Origin, string> = {
    signal: 'Signal',
    gchat: 'Google Chat',
    irc: 'IRC',
    telegram: 'Telegram',
  };
  readonly origins: readonly Origin[] = ['signal', 'gchat', 'irc', 'telegram'];

  // `?origin` filters the list.
  private params = toSignal(this.route.queryParamMap);
  readonly originFilter = computed<Origin | 'all'>(() => {
    const o = this.params()?.get('origin');
    // Matched against the list, so a new origin is filterable automatically.
    return this.origins.find((known) => known === o) ?? 'all';
  });

  // The open conversation, from the router, so the list can highlight it and
  // the phone layout switch panes.
  private navEnd = toSignal(this.router.events.pipe(filter((e) => e instanceof NavigationEnd)));
  readonly active = computed<{ origin: string; id: string } | null>(() => {
    this.navEnd();
    const pm = this.leaf().snapshot.paramMap; // typed get() → string | null
    const origin = pm.get('origin');
    const id = pm.get('id');
    return origin != null && id != null ? { origin, id } : null;
  });

  // Search overlays the list and is not URL state. `switchMap` cancels a stale
  // request, so a slow answer cannot land after a newer one.
  readonly query = signal('');
  readonly results = signal<SearchHit[] | null>(null);
  readonly searching = signal(false);
  /** The search failed, distinct from no hits: "No matches." is a claim. */
  readonly searchFailed = signal(false);
  private search$ = new Subject<string>();

  readonly visibleConversations = computed(() => {
    const f = this.originFilter();
    const list = this.conversations();
    return f === 'all' ? list : list.filter((c) => c.origin === f);
  });

  constructor() {
    this.swUpdates.start();
    this.store.init();
    this.telemetry.init();

    // Re-read the list on returning to it.
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

    // Also when the app returns to the foreground, which no navigation marks:
    // the list would otherwise show whatever it last rendered. Unconditional,
    // since on a wide screen the list sits beside the thread.
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
    // Update `?origin`, keeping the open conversation.
    void this.router.navigate([], {
      relativeTo: this.leaf(),
      queryParams: { origin: f === 'all' ? null : f },
      queryParamsHandling: 'merge',
    });
  }

  /** Open a conversation, keeping the origin filter and clearing `?from` so it
   *  opens at the latest page. */
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


  /** Open a search result on the message it found. Routes from the hit, which
   *  carries origin and id, so it works before the list loads. `from: null`: a
   *  scroll position from another conversation must not come along. */
  openHit(h: SearchHit): void {
    void this.router.navigate(['/conversation', h.origin, h.conversation_id], {
      queryParams: { at: h.cursor, from: null },
      queryParamsHandling: 'merge',
    });
  }

  /** What to call the conversation a hit is in: `MessagesStore.title` when the
   *  list has it, else the hit's own name. */
  hitTitle(h: SearchHit): string {
    const c = this.store.find(h.origin, h.conversation_id);
    if (c) return this.title(c);
    // An empty name after `trim` is no name; `??` would pass `''` through.
    const name = h.conversation_name?.trim() ?? '';
    return name.length > 0 ? name : 'Conversation';
  }

  /** Which archive a hit came from, and on which network, as the conversation
   *  list shows: one IRC target can exist on two networks. */
  hitOrigin(h: SearchHit): string {
    const network = this.store.find(h.origin, h.conversation_id)?.network;
    return network ? `${this.originLabels[h.origin]} ${network}` : this.originLabels[h.origin];
  }

  title(c: Conversation): string {
    return this.store.title(c);
  }

  signOut(): void {
    this.api.logout().subscribe(() => (window.location.href = '/'));
  }
}
