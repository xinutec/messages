import { HttpClient } from '@angular/common/http';
import { Injectable, inject } from '@angular/core';
import { Observable } from 'rxjs';
import { Conversation, LinkImageState, Me, MessagesPage, Origin, SearchHit, SendResult, MediaState } from './models';

/** Client for the messages backend: same-origin in prod, via proxy.conf.json in
 *  `ng serve`. */
@Injectable({ providedIn: 'root' })
export class MessagesApi {
  private http = inject(HttpClient);

  me(): Observable<Me> {
    return this.http.get<Me>('/api/me');
  }
  logout(): Observable<unknown> {
    return this.http.post('/logout', {});
  }

  /** Ask for a link's picture, by the id the page offered. */
  requestLinkImage(id: string): Observable<LinkImageState> {
    return this.http.post<LinkImageState>(
      `/api/link-images/${encodeURIComponent(id)}/request`,
      {},
    );
  }

  /** Has an asked-for attachment arrived? */
  telegramMediaState(id: string): Observable<MediaState> {
    return this.http.get<MediaState>(
      `/api/telegram-media/${encodeURIComponent(id)}/state`,
    );
  }

  /** Ask the Telegram feed for an unfetched attachment, by message id. 204
   *  whether or not anything was queued. */
  requestTelegramMedia(id: string): Observable<void> {
    return this.http.post<void>(
      `/api/telegram-media/${encodeURIComponent(id)}/request`,
      {},
    );
  }

  conversations(): Observable<Conversation[]> {
    return this.http.get<Conversation[]>('/api/conversations');
  }

  /** The list as the service worker last kept it (ngsw-config's `api` group),
   *  or `null`: shown at once, until the network's answer replaces it. */
  keptConversations(): Promise<Conversation[] | null> {
    return kept<Conversation[]>('/api/conversations');
  }

  /** The newest page as the service worker last kept it, or `null`; the URL is
   *  the one `messages` asks for, so the worker's key is the same. */
  keptMessages(origin: Origin, id: string, limit = 100): Promise<MessagesPage | null> {
    return kept<MessagesPage>(`/api/conversations/${origin}/${encodeURIComponent(id)}/messages?limit=${limit}`);
  }

  /** One page of a conversation. `dir`:
   *
   *  - `older` (the default): strictly before the cursor;
   *  - `newer`: strictly after it, for scrolling forward;
   *  - `at`: the cursor's own row and after, for landing. */
  messages(
    origin: Origin,
    id: string,
    cursor?: string,
    limit = 100,
    dir?: 'older' | 'newer' | 'at',
    /** The day to land on: local midnight in epoch ms, converted by the server. */
    on?: number,
  ): Observable<MessagesPage> {
    const params: Record<string, string> = { limit: String(limit) };
    if (cursor != null) params['cursor'] = cursor;
    if (dir != null) params['dir'] = dir;
    if (on != null) params['on'] = String(on);
    return this.http.get<MessagesPage>(
      `/api/conversations/${origin}/${encodeURIComponent(id)}/messages`,
      { params },
    );
  }

  /** Say something in an IRC conversation, through irssi. The recipient comes
   *  from the conversation, server-side. A refusal resolves with `sent: false`;
   *  only a transport failure errors. */
  send(origin: Origin, id: string, text: string): Observable<SendResult> {
    return this.http.post<SendResult>(
      `/api/conversations/${origin}/${encodeURIComponent(id)}/send`,
      { text },
    );
  }

  /** Search everywhere, or inside one conversation: `origin` and `id` together
   *  or not at all (the server 404s half a scope). */
  search(q: string, scope?: { origin: Origin; id: string }): Observable<SearchHit[]> {
    // A flat record: a union would give `origin?: undefined`, which
    // HttpClient's params type rejects.
    const params: Record<string, string> = { q };
    if (scope) {
      params['origin'] = scope.origin;
      params['id'] = scope.id;
    }
    return this.http.get<SearchHit[]>('/api/search', { params });
  }
}

/** What the service worker last stored for `url`, read through the browser's
 *  Cache API: the page renders it before the network answers, and the policy
 *  (network first) would otherwise show nothing until then. `null` with no
 *  worker, no entry, or an entry that does not read. */
async function kept<T>(url: string): Promise<T | null> {
  if (typeof caches === 'undefined') return null;
  try {
    const res = await caches.match(url);
    if (res == null) return null;
    // eslint-disable-next-line @typescript-eslint/no-unsafe-type-assertion -- the worker stored what the typed request above returned: the same trust HttpClient's generic gives the network's answer
    return (await res.json()) as T;
  } catch {
    return null;
  }
}
