# messages — Signal, Google Chat, IRC and Telegram archive

[`archiver/`](archiver/README.md) writes all four origins into the **`signal`
MariaDB** on the isis k3s cluster. The viewer (this crate and `frontend/`) reads
them, and can reply on IRC through the irssi that holds the connections.

```
 Browser ──VPN/login──▶ messages.xinutec.org (isis, ns: signal)
                            │  Rust/axum: Nextcloud OAuth2 (identity) + sessions
                            │  + API over the archive, + IRC send via ssh
                            ▼
                        signal MariaDB  ─ messages / conversations / reactions        (Signal)
                                        ├ gchat_messages / gchat_conversations…        (Google Chat)
                                        ├ irc_messages / irc_conversations             (IRC)
                                        └ telegram_messages / telegram_conversations…  (Telegram)
```

IRC shows only what was said: every read restricts to `kind IN ('message',
'action')`, leaving out joins, parts and notices, and irssi's server-notice
window (`is_status`) is not listed.

## Security model
1. **Nextcloud login and allow-list, the real gate.** OAuth2 identity against
   `dash.xinutec.org`; an authenticated user not in `ALLOWED_USERS` gets 403.
2. **VPN-only by DNS.** `messages.xinutec.org` resolves to isis's WireGuard IP.
   This is obscurity: the ingress also answers on the public IP.

An ingress `whitelist-source-range` would make the second layer real, if client
source IPs survive k3s servicelb; check before relying on it.

## Components
- `archiver/` — the ingesters and importers, and the schema's migrations. Its
  README covers them.
- `src/` — the viewer's backend. `nextcloud/identity.rs` and `session.rs` are the login;
  `routes/auth.rs` adds the allow-list; `archive.rs` is the origin-normalising
  query layer; `irc_send.rs` sends; `config.rs` builds the database connection
  from `DB_*`, so the app reuses `signal-secret`. The app owns only `sessions`
  and `link_images` (`src/db.rs`).
- `src/bin/link-fetch.rs` — the link-picture fetch service, the only part that
  reaches the internet. It holds no credentials, database or disk.
- `frontend/` — Angular: conversation list with origin filter, thread view, and
  an IRC composer. `src/app/thread-window.ts` keeps a window of a long thread in
  the DOM; `src/app/copy-log.ts` copies a selection as an irssi log (the inverse
  of `archiver/src/irclog.rs`); `src/app/attachment.ts` names attachments for both
  screen and clipboard. `src/app/generated/` is written by ts-rs
  (`scripts/gen-types.sh`) and imported through `src/app/models.ts`.
- `Dockerfile` — `xinutec/messages:latest`, with both viewer binaries;
  `archiver/Dockerfile` is `xinutec/signal-archiver:latest`. Both build from the
  repository root, since the Cargo workspace spans the two crates.

## API (all require a session)
- `GET /api/me` — current user.
- `GET /api/conversations` — every conversation, newest activity first.
- `GET /api/conversations/{origin}/{id}/messages?cursor=&limit=&dir=&on=` — one
  page, oldest first. `cursor` is opaque, `(native_ts, id)`, so rows sharing a
  timestamp are never skipped. `dir` is `older` (default), `newer`, or `at` (the
  cursor's own row and after, for landing). `on` is a day's local midnight in
  epoch ms; the server converts it to the origin's unit.
- `GET /api/search?q=[&origin=&id=]` — substring search, everywhere or in one
  conversation. Each hit carries a cursor that lands on it.
- `POST /api/conversations/irc/{id}/send` — say something through irssi. The
  body is the text only; the recipient comes from `{id}`. Other origins 404:
  Signal by decision (see `routes/api.rs`), Telegram because sending there is not
  designed yet.
- `GET /api/attachments/{id}`, `/api/gchat-attachments/{id}`,
  `/api/telegram-media/{id}` — held bytes, one route per origin since attachment
  ids are per origin. `POST /api/telegram-media/{id}/request` queues an
  unfetched Telegram file; `GET …/state` reports its progress.
- `GET /api/link-images/{id}`, `POST /api/link-images/{id}/request` — a picture
  for a link in a message, by the URL's hash.
- `POST /api/telemetry` — client events into the server log. Always 204.

`{origin}` is `signal`, `gchat`, `irc` or `telegram`. `{id}` is the Signal
`thread_id`, the Google Chat `group_id`, the `irc_conversations.id`, or
Telegram's folded peer id (see v15 in `archiver/src/db.rs`).

## Local dev
```
# backend (needs the database; tunnel signal-db or use a local MariaDB)
DB_HOST=127.0.0.1 DB_PORT=3306 DB_NAME=signal DB_USER=… DB_PASSWORD=… \
NC_BASE_URL=https://dash.xinutec.org NC_CLIENT_ID=… NC_CLIENT_SECRET=… \
NC_REDIRECT_URI=http://localhost:4200/auth/callback \
SESSION_SECRET=$(openssl rand -hex 32) ALLOWED_USERS=pippijn \
  cargo run
# frontend (proxies /api, /login, /auth, /logout to :8080)
cd frontend && pnpm install && pnpm start  # http://localhost:4200
```

## Deploy (isis, namespace `signal`)
Manifests are in the home monorepo (`xinutec/pippijn`, `code/kubes/messages/k8s/`).
Push to main, wait for CI to build both images, then run
`code/kubes/deploy.sh messages` (the viewer) or `code/kubes/deploy.sh signal`
(the archiver) from that checkout. It refuses unless the
manifests are committed and pushed and isis's checkout matches, and restarts a
`:latest` workload only when the registry has a newer image.

One-time setup, in case it must be redone: register the OAuth2 client in
Nextcloud admin (redirect URI `https://messages.xinutec.org/auth/callback`); put
a Cloudflare `Zone:DNS:Edit` token in `cert-manager` as `cloudflare-api-token`
and apply `00-letsencrypt-dns-issuer.yaml` (isis needs DNS-01); `messages →
10.100.0.2` is in `code/dns`; `NC_CLIENT_ID=… NC_CLIENT_SECRET=…
./k8s/secret.sh` writes the session key and OAuth client.

## Tests
`gate.dhall` is the gate and the pre-commit hook:

```sh
nix run ../dev-lint#gate -- . gate.json
```

`gate.json` is rendered from the Dhall and committed; one check re-renders and
diffs it.

- `tests/archive.rs` — pure units, plus end-to-end tests against a fixture in a
  throwaway MariaDB. Those need `MESSAGES_TEST_DATABASE_URL` and skip without
  it; the gate's test row starts one via dev-lint's `with-test-db`.
- `tests/access.rs`, `tests/session_cookie.rs`, `tests/error_responses.rs` — the
  allow-list fails closed, sessions cannot be forged, and a 500 says nothing
  about itself.
- `tests/api_routes.rs` — requests through the real router reach the handlers
  behind the auth extractor. It touches no archive table, since
  `tests/archive.rs` recreates them in the same database.
- Frontend unit tests (`pnpm test`, vitest) cover logic. jsdom has no layout,
  fonts, or real Selection API, and does not submit forms on Enter, so
  `pnpm run ui-check` runs the Playwright suite against the production build:
  phone-width layout, copy, scrolling, routing, the Android keyboard and a real
  IME composition. Treat vitest as no evidence about the composer.
- `archiver/tests/` — the archiver's suite, in its own database
  (`SIGNAL_TEST_DATABASE_URL`, its own gate row): it applies the real
  migrations, which the fixture above would collide with.

## One concept, several readers
Each field below is interpreted in more than one place; add a row when a field
gains another reader.

| field | Rust | thread.html | copy-log.ts | search |
| --- | --- | --- | --- | --- |
| `deleted` | Signal and Telegram; always `false` for Google Chat and IRC | hidden behind a click, body and attachments | `(deleted)` only, attachments included | listed, with `(deleted)` for the snippet |
| `edited` | Signal and Telegram, by different mechanisms; Telegram's also honours `edit_hide` | `edited` tag | ` (edited)` on the last line | not shown |
| `edits` | Signal: revision rows via `edit_of_ts`; Telegram: `telegram_message_edits` | behind the `edited` tag | not shown | not shown |
| `kind` | IRC, and Telegram service events as `action` | `* ` before the body | `HH:MM  * nick ` prefix | not shown |
| `entities` | Telegram entities; Signal text styles mapped to the same kinds, from an edited message's newest revision | formatted runs, overlaps combined | plain text | not shown |
| `is_outgoing` | all origins | `.out` class | nothing; the sender's name carries it | not shown |

The server sends retracted text; every reader hides it until asked. A revealed
message still copies as `(deleted)`, since the log is built from the model.

`MessagesStore.title` names a conversation for the list, the search results and,
before the list loads, the hit's own name; the thread header says
"Conversation" until then.

A message with neither body nor attachments draws an empty bubble but produces no
copied line.

`?at` carries a cursor and means *put me here*; `?from` carries a scroll position
and means *I was here*. `at` wins on load, and the first scroll replaces it with
`from` and clears the landed-message marker.

Delivery state (`sent`, `delivered`, `read`) appears only where the archive can
say: outgoing Signal and Telegram messages sent after capture began. Telegram
reports a read position and names nobody; Signal reports per-person receipts, so
a group says `read by 2` rather than `read`, and my own linked device's read
sync is not counted.

Sending is optional: without a usable key the app serves the archive and refuses
to send.

## Known limits
- Signal reactions are distinct current authors per emoji, so a same-author
  add-then-remove within one page is missed.
- Google Chat reactors are named only as far as gchat-archive's second capture
  has reached; Telegram truncates its reactor list. `count` stays authoritative.
- A custom-emoji Telegram reaction has no characters to draw and is left out.
- Signal link previews show title and description; a preview image is not yet
  captured (#1693).
- Telegram secret chats are device-local and absent.
- A Telegram message edited before the archive saw it has no earlier versions:
  Telegram serves only the current text.
- A Google Chat picture's bytes are held only if a harvest fetched them; the
  download URL needs Pippijn's session.
- Attachments are read whole into memory to serve, against the pod's memory limit.
- Copying reaches only the rendered window (400 messages). A select-all of a
  longer conversation ends with `--- copied 400 of N messages; the rest were not
  loaded on screen`.
