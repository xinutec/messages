import { ApplicationConfig, LOCALE_ID, provideBrowserGlobalErrorListeners, provideZonelessChangeDetection } from '@angular/core';
import { provideHttpClient, withFetch } from '@angular/common/http';
import { provideRouter, withComponentInputBinding } from '@angular/router';

import { registerLocaleData } from '@angular/common';
import localeEnGb from '@angular/common/locales/en-GB';

import { routes } from './app.routes';

// Angular defaults LOCALE_ID to `en-US` whatever the browser is set to, so every
// `| date` rendered US dates to a UK reader. It is a different knob from
// `toLocaleString()`, which follows the browser and was already right on a phone
// — the two can disagree inside one render. Registering the locale data is
// required as well as naming the id: without it the pipe throws on any format
// needing month or day names.
registerLocaleData(localeEnGb);

export const appConfig: ApplicationConfig = {
  providers: [
    provideBrowserGlobalErrorListeners(),
    { provide: LOCALE_ID, useValue: 'en-GB' },
    provideZonelessChangeDetection(),
    provideHttpClient(withFetch()),
    // A real routes table (see app.routes.ts); route params bind to the Thread's
    // inputs so the URL is the source of truth for the open conversation.
    provideRouter(routes, withComponentInputBinding()),
  ],
};
