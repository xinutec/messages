// What a reader asked the archive to fetch: a Telegram file the feed has not
// downloaded, and the picture behind a link. Each lands in the thread's messages
// in place when it arrives; see thread.ts.

import { WritableSignal, signal } from '@angular/core';

import { MessagesApi } from './messages-api';
import { Attachment, LinkOffer, Message } from './models';

/** How often a fetch in flight is asked about. */
const MEDIA_WATCH_INTERVAL_MS = 4000;

/** How long to watch a requested fetch before giving up quietly; reopening the
 *  conversation asks again. */
const MEDIA_WATCH_LIMIT_MS = 15 * 60 * 1000;

export class FetchRequests {
  /** Requests in flight, by attachment or link id. */
  private readonly asking = signal<ReadonlySet<string>>(new Set());

  /** Attachment id → interval, for requested fetches being watched. */
  private readonly watching = new Map<string, number>();

  constructor(
    private readonly api: MessagesApi,
    private readonly messages: WritableSignal<Message[]>,
  ) {}

  isAsking(id: string): boolean {
    return this.asking().has(id);
  }

  /** Forget every request, for a fresh conversation or a destroyed thread. */
  reset(): void {
    for (const id of [...this.watching.keys()]) this.stopWatching(id);
    this.asking.set(new Set());
  }

  /** Ask the feed for an unfetched attachment. The fetch is queued, so the answer
   *  is watched for rather than awaited. */
  requestMedia(a: Attachment): void {
    this.setAsking(a.id, true);
    this.api.requestTelegramMedia(a.id).subscribe({
      // 204 either way.
      next: () => this.watchMedia(a.id),
      error: () => this.setAsking(a.id, false),
    });
  }

  /** Watch fetches already in flight when a page arrives, including ones asked
   *  for earlier or by someone else. */
  watchInFlight(msgs: readonly Message[]): void {
    for (const m of msgs) {
      for (const a of m.attachments) {
        if (a.fetch === 'wanted') this.watchMedia(a.id);
      }
    }
  }

  /** A reader asked for a link's picture; the answer comes back on this request. */
  requestLinkImage(offer: LinkOffer): void {
    this.setAsking(offer.id, true);
    this.api.requestLinkImage(offer.id).subscribe({
      next: (state) => {
        this.setAsking(offer.id, false);
        if (state.state === 'ok' && state.content_type) {
          this.landLinkImage(offer.id, state.content_type);
        } else {
          // Not a picture, or unreachable: the control goes.
          this.dropOffer(offer.id);
        }
      },
      error: () => {
        this.setAsking(offer.id, false);
        this.dropOffer(offer.id);
      },
    });
  }

  /** Poll an asked-for attachment until it arrives or fails, giving up quietly
   *  after `MEDIA_WATCH_LIMIT_MS`. */
  private watchMedia(id: string): void {
    if (this.watching.has(id)) return;
    const started = Date.now();
    const done = (): void => {
      this.stopWatching(id);
      this.setAsking(id, false);
    };
    const tick = window.setInterval(() => {
      if (Date.now() - started > MEDIA_WATCH_LIMIT_MS) {
        done();
        return;
      }
      this.api.telegramMediaState(id).subscribe({
        next: (state) => {
          if (state.available) {
            this.landMedia(id, state.content_type);
            done();
          } else if (state.fetch === 'failed') {
            // Back to an offer the reader can ask again.
            done();
            this.updateAttachment(id, (a) => ({ ...a, fetch: 'failed' }));
          }
        },
        error: done,
      });
    }, MEDIA_WATCH_INTERVAL_MS);
    this.watching.set(id, tick);
  }

  private stopWatching(id: string): void {
    const tick = this.watching.get(id);
    if (tick !== undefined) window.clearInterval(tick);
    this.watching.delete(id);
  }

  /** The file arrived: flip the attachment in the model so it draws. */
  private landMedia(id: string, contentType: string | null): void {
    this.updateAttachment(id, (a) => {
      const type = contentType ?? a.content_type;
      return { ...a, available: true, fetch: null, content_type: type };
    });
  }

  private updateAttachment(id: string, change: (a: Attachment) => Attachment): void {
    this.messages.update((cur) =>
      cur.map((m) => ({ ...m, attachments: m.attachments.map((a) => (a.id === id ? change(a) : a)) })),
    );
  }

  /** Swap the offer for the picture, in place. */
  private landLinkImage(id: string, contentType: string): void {
    this.messages.update((cur) =>
      cur.map((m) => {
        const offer = m.link_offers.find((o) => o.id === id);
        if (!offer) return m;
        return {
          ...m,
          link_offers: m.link_offers.filter((o) => o.id !== id),
          link_images: [...m.link_images, { url: offer.url, id, content_type: contentType }],
        };
      }),
    );
  }

  private dropOffer(id: string): void {
    this.messages.update((cur) =>
      cur.map((m) =>
        m.link_offers.some((o) => o.id === id)
          ? { ...m, link_offers: m.link_offers.filter((o) => o.id !== id) }
          : m,
      ),
    );
  }

  private setAsking(id: string, on: boolean): void {
    this.asking.update((cur) => {
      const next = new Set(cur);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });
  }
}
