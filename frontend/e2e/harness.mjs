// This app's half of the shared phone-width harness (@xinutec/ui-harness),
// read by playwright.config.ts and the harness's static server. The port is
// derived from `app`.

/** @type {import('@xinutec/ui-harness/config').HarnessSpec} */
export default {
  app: 'messages',
  dist: 'dist/messages-web/browser',
  // A fallback only; the specs mock every route. An unmocked run still renders
  // the shell.
  api: {
    '/api/me': { user_id: 'test', display_name: 'Test' },
    '/api/conversations': [],
  },
};
