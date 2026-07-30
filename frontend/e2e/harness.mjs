// The app-specific half of the shared phone-width harness (@xinutec/ui-harness).
// Read by BOTH playwright.config.ts and the harness's static server, so there is
// one place to say what this app is and no port to keep in step — the port is
// allocated from `app`.

/** @type {import('@xinutec/ui-harness/config').HarnessSpec} */
export default {
  app: 'messages',
  dist: 'dist/messages-web/browser',
  // Fallback stub only — the specs page.route everything. Real prod is the Rust
  // backend. Signed-in user + no conversations, so an un-mocked run still
  // renders the shell instead of an error screen.
  api: {
    '/api/me': { user_id: 'test', display_name: 'Test' },
    '/api/conversations': [],
  },
};
