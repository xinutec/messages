import { ErrorHandler, ApplicationConfig, LOCALE_ID, isDevMode, provideBrowserGlobalErrorListeners, provideZonelessChangeDetection } from '@angular/core';
import { provideServiceWorker } from '@angular/service-worker';
import { provideHttpClient, withFetch, withInterceptors } from '@angular/common/http';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { provideNativeDateAdapter } from '@angular/material/core';

import { registerLocaleData } from '@angular/common';
import localeEnGb from '@angular/common/locales/en-GB';

import { routes } from './app.routes';
import { TelemetryErrorHandler, failedRequestInterceptor } from './error-reporting';

// Angular defaults LOCALE_ID to `en-US` whatever the browser says, and the
// `date` pipe follows it. The locale data must be registered too, or month and
// day names throw.
registerLocaleData(localeEnGb);

export const appConfig: ApplicationConfig = {
  providers: [
    { provide: ErrorHandler, useClass: TelemetryErrorHandler },
    provideBrowserGlobalErrorListeners(),
    { provide: LOCALE_ID, useValue: 'en-GB' },
    // The calendar's dates, as plain `Date`s in the same locale.
    provideNativeDateAdapter(),
    provideZonelessChangeDetection(),
    provideHttpClient(withFetch(), withInterceptors([failedRequestInterceptor])),
    // Route params bind to the Thread's inputs; the URL is the source of truth.
    provideRouter(routes, withComponentInputBinding()),
    // Registered once the app settles, so it cannot compete with the first
    // paint. Disabled in dev, where the build changes under it.
    provideServiceWorker('ngsw-worker.js', {
      enabled: !isDevMode(),
      registrationStrategy: 'registerWhenStable:30000',
    }),
  ],
};
