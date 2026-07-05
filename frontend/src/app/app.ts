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

import { Subject, catchError, filter, of, switchMap } from 'rxjs';

import { MessagesApi } from './messages-api';
import { MessagesStore } from './messages-store';
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

  readonly me = this.store.me;
  readonly loading = this.store.loading;
  readonly conversations = this.store.conversations;

  // ?origin filters the list — view-state on the route, like health's ?date.
  private params = toSignal(this.route.queryParamMap);
  readonly originFilter = computed<Origin | 'all'>(() => {
    const o = this.params()?.get('origin');
    return o === 'signal' || o === 'gchat' ? o : 'all';
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
  private search$ = new Subject<string>();

  readonly visibleConversations = computed(() => {
    const f = this.originFilter();
    const list = this.conversations();
    return f === 'all' ? list : list.filter((c) => c.origin === f);
  });

  constructor() {
    this.store.init();
    this.search$
      .pipe(
        switchMap((q) => this.api.search(q).pipe(catchError(() => of<SearchHit[]>([])))),
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
    this.search$.next(q);
  }

  clearSearch(): void {
    this.query.set('');
    this.results.set(null);
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
