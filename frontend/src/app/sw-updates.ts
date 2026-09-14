import { Injectable, inject } from '@angular/core';
import { SwUpdate, VersionReadyEvent } from '@angular/service-worker';
import {
  type PagePort,
  type ServiceWorkerPort,
  SwUpdates,
  type UpdateOutcome,
} from '@xinutec/ui-harness/sw-updates';
import { filter } from 'rxjs';

/** Session-scoped, so it survives the very reload it guards. */
const RECOVERY_KEY = 'messages.sw-recovery-attempted';

/**
 * Self-update, so the app cannot go on running a build the server has replaced.
 *
 * ⚠ **THIS APP HAD NOTHING, AND IT SHOWED.** On 2026-09-14 the Android WebView was
 * running `main-IZLCFSTD.js` while the server served `main-YRMPMXEH.js` — an
 * `index.html` cached despite `cache-control: no-cache`, naming a bundle from before
 * Telegram existed. New data, old code: every Telegram conversation drew with a blank
 * origin label and the filter row had no Telegram button. A cold start did not fix it;
 * only a cache-bypassing reload did.
 *
 * `DL-NGSW-ABSENT` had been reporting this app for weeks (#1384), with `life` and
 * `fleetwatch` named as the worked examples. The control existed; nobody had applied it.
 *
 * The rules live in `@xinutec/ui-harness/sw-updates`, written and debugged in `life`.
 * The one that matters most here is the re-check on becoming visible: ngsw only
 * re-checks at a navigation, and the WebView shell reopens where it left off rather
 * than navigating — so a phone that is never cold-started never performs one.
 *
 * This class is only the adapter. The policy is shared and unit-tested against a fake;
 * what is here is the Angular wiring, which is the part a test cannot reach.
 */
@Injectable({ providedIn: 'root' })
export class AppSwUpdates {
  private readonly sw = inject(SwUpdate);

  private readonly serviceWorker: ServiceWorkerPort = ((sw: SwUpdate) => ({
    // Bound to a local, not `this`: an object-literal getter does not capture the
    // enclosing `this` lexically, and a copied boolean would freeze `isEnabled` at
    // construction when start() must read the live value.
    get isEnabled(): boolean {
      return sw.isEnabled;
    },
    onVersionReady: (handler: () => void): void => {
      sw.versionUpdates
        .pipe(filter((event): event is VersionReadyEvent => event.type === 'VERSION_READY'))
        .subscribe(() => handler());
    },
    onUnrecoverable: (handler: () => void): void => {
      // The cached build is broken and the server no longer holds the files to
      // repair it — what a roll-forward deploy of :latest leaves a client whose
      // cache was evicted meanwhile. Only a fresh load escapes.
      sw.unrecoverable.subscribe(() => handler());
    },
    checkForUpdate: () => sw.checkForUpdate(),
    activateUpdate: () => sw.activateUpdate(),
  }))(this.sw);

  private readonly page: PagePort = {
    get hidden(): boolean {
      return document.visibilityState === 'hidden';
    },
    onVisibilityChange: (handler: () => void): void => {
      document.addEventListener('visibilitychange', handler);
    },
    recoveryAttempted: () => sessionStorage.getItem(RECOVERY_KEY) !== null,
    markRecoveryAttempted: () => sessionStorage.setItem(RECOVERY_KEY, '1'),
    reload: () => document.location.reload(),
    now: () => Date.now(),
  };

  private readonly policy = new SwUpdates(this.serviceWorker, this.page);

  start(): void {
    this.policy.start();
  }

  /** Manual "check for updates", for a settings screen to call. */
  checkNow(): Promise<UpdateOutcome> {
    return this.policy.checkNow();
  }
}
