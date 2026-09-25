import { Injectable, inject } from '@angular/core';
import { SwUpdate, VersionReadyEvent } from '@angular/service-worker';
import { type PagePort, type ServiceWorkerPort, SwUpdates } from '@xinutec/ui-harness/sw-updates';
import { filter } from 'rxjs';

/** Session-scoped, so it survives the very reload it guards. */
const RECOVERY_KEY = 'messages.sw-recovery-attempted';

/**
 * Self-update, so the app cannot keep running a build the server has replaced
 * (an Android WebView can keep a stale `index.html` despite `no-cache`). The
 * policy is `@xinutec/ui-harness/sw-updates`; this is the Angular wiring. It
 * re-checks on becoming visible, since the WebView reopens without navigating.
 */
@Injectable({ providedIn: 'root' })
export class AppSwUpdates {
  private readonly sw = inject(SwUpdate);

  private readonly serviceWorker: ServiceWorkerPort = ((sw: SwUpdate) => ({
    // A local, not `this`: an object-literal getter does not capture `this`, and
    // `start()` must read the live value.
    get isEnabled(): boolean {
      return sw.isEnabled;
    },
    onVersionReady: (handler: () => void): void => {
      sw.versionUpdates
        .pipe(filter((event): event is VersionReadyEvent => event.type === 'VERSION_READY'))
        .subscribe(() => handler());
    },
    onUnrecoverable: (handler: () => void): void => {
      // The cached build is broken and the server no longer has its files; only
      // a fresh load recovers.
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
}
