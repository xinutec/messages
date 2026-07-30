import { HttpClient } from '@angular/common/http';
import { Injectable, inject } from '@angular/core';
import { Observable } from 'rxjs';
import { Conversation, Me, MessagesPage, Origin, SearchHit, TelemetryEvent } from './models';

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

  messages(origin: Origin, id: string, cursor?: string, limit = 100): Observable<MessagesPage> {
    const params: Record<string, string> = { limit: String(limit) };
    if (cursor != null) params['cursor'] = cursor;
    return this.http.get<MessagesPage>(
      `/api/conversations/${origin}/${encodeURIComponent(id)}/messages`,
      { params },
    );
  }

  search(q: string): Observable<SearchHit[]> {
    return this.http.get<SearchHit[]>('/api/search', { params: { q } });
  }
}
