import { HttpClient } from '@angular/common/http';
import { Injectable, inject } from '@angular/core';
import { Observable } from 'rxjs';
import { Conversation, LinkImageState, Me, MessagesPage, Origin, SearchHit, SendResult, TelemetryEvent, MediaState } from './models';

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

  /** Ask for a picture behind a link. The id is the one the page offered — the
   *  browser never names an address, which is what keeps this from being a
   *  fetch-anything endpoint. */
  requestLinkImage(id: string): Observable<LinkImageState> {
    return this.http.post<LinkImageState>(
      `/api/link-images/${encodeURIComponent(id)}/request`,
      {},
    );
  }

  /** Has an asked-for attachment arrived? The POST cannot say — the fetch happens
   *  in another process, minutes later for a large file. */
  telegramMediaState(id: string): Observable<MediaState> {
    return this.http.get<MediaState>(
      `/api/telegram-media/${encodeURIComponent(id)}/state`,
    );
  }

  /** Ask the Telegram feed for an attachment it has not fetched. The id is the
   *  message's, which is what the page already holds — as with a link picture, the
   *  browser never names a file, so this cannot be turned into a fetch-anything
   *  endpoint. 204 whether or not anything was queued. */
  requestTelegramMedia(id: string): Observable<void> {
    return this.http.post<void>(
      `/api/telegram-media/${encodeURIComponent(id)}/request`,
      {},
    );
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

  /** Search everywhere, or inside one conversation.
   *
   *  ⚠ **BOTH `origin` AND `id` OR NEITHER.** The server refuses half a scope
   *  rather than widening to a global search, so a caller that sends one without
   *  the other gets a 404 instead of answers from everywhere that look like the
   *  scope silently not working. */
  search(q: string, scope?: { origin: Origin; id: string }): Observable<SearchHit[]> {
    // ⚠ Typed as a flat record rather than a ternary of two object literals:
    // the union gives the unscoped branch `origin?: undefined`, which HttpClient's
    // params type rejects outright.
    const params: Record<string, string> = { q };
    if (scope) {
      params['origin'] = scope.origin;
      params['id'] = scope.id;
    }
    return this.http.get<SearchHit[]>('/api/search', { params });
  }
}
