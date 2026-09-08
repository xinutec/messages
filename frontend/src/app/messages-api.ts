import { HttpClient } from '@angular/common/http';
import { Injectable, inject } from '@angular/core';
import { Observable } from 'rxjs';
import { Conversation, Me, MessagesPage, Origin, SearchHit, SendResult, TelemetryEvent } from './models';

/** Thin client over the messages backend. Same-origin in prod; via the dev
 *  proxy (proxy.conf.json) in `ng serve`. Session cookie rides along. */
@Injectable({ providedIn: 'root' })
export class MessagesApi {
  private http = inject(HttpClient);

  me(): Observable<Me> {
    return this.http.get<Me>('/api/me');
  }
  logout(): Observable<unknown> {
    return this.http.post('/logout', {});
  }

  /** Send a batch of client events to be logged. Fire-and-forget at the call
   *  site: a trace that surfaces its own failures interferes with the app it
   *  observes. */
  sendTelemetry(events: readonly TelemetryEvent[]): Observable<void> {
    return this.http.post<void>('/api/telemetry', events);
  }

  conversations(): Observable<Conversation[]> {
    return this.http.get<Conversation[]>('/api/conversations');
  }

  /** One page of a conversation.
   *
   *  `dir` says which way the page runs from `cursor`:
   *
   *  - `older` (the default, and what the whole app did before #1401)
   *  - `newer` — STRICTLY after the cursor, for scrolling forward, where the
   *    caller already holds that row;
   *  - `at` — the cursor's own row and forward, for a LANDING.
   *
   *  ⚠ The last distinction is not pedantry. A landing built from `older` +
   *  `newer` skips the row it is aimed at, because both are strict; that
   *  shipped on 2026-09-08 and put the reader one message past the hit. */
  messages(
    origin: Origin,
    id: string,
    cursor?: string,
    limit = 100,
    dir?: 'older' | 'newer' | 'at',
  ): Observable<MessagesPage> {
    const params: Record<string, string> = { limit: String(limit) };
    if (cursor != null) params['cursor'] = cursor;
    if (dir != null) params['dir'] = dir;
    return this.http.get<MessagesPage>(
      `/api/conversations/${origin}/${encodeURIComponent(id)}/messages`,
      { params },
    );
  }

  /** Say something in an IRC conversation, as Pippijn, through irssi.
   *
   *  ⚠ Only the text is sent. Who receives it is decided by the conversation in
   *  the URL and looked up server-side, so this cannot address anyone the
   *  archive has not already seen — and irssi has the final say, refusing any
   *  target it has no tab open with.
   *
   *  A refused send resolves normally with `sent: false` and a reason; only a
   *  transport failure errors. */
  send(origin: Origin, id: string, text: string): Observable<SendResult> {
    return this.http.post<SendResult>(
      `/api/conversations/${origin}/${encodeURIComponent(id)}/send`,
      { text },
    );
  }

  search(q: string): Observable<SearchHit[]> {
    return this.http.get<SearchHit[]>('/api/search', { params: { q } });
  }
}
