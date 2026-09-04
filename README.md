# messages — Signal + Google Chat + IRC archive viewer

A web UI for the message archive stored in the **`signal` MariaDB** on the isis
k3s cluster — three origins ([Signal](../signal) live + history, the imported
Google Chat tables, and irssi's autologs). Reading is all of it bar one thing:
IRC conversations can be replied to, through the irssi that already holds the
connections. Same per-service pattern as `life`/`health`.

```
 Browser ──VPN/login──▶ messages.xinutec.org (isis, ns: signal)
                            │  Rust/axum: Nextcloud OAuth2 (identity) + sessions
                            │  + API over the archive, + IRC send via ssh
                            ▼
                        signal MariaDB  ─ messages / conversations / reactions   (Signal)
                                        ├ gchat_messages / gchat_conversations…   (Google Chat)
                                        └ irc_messages / irc_conversations         (IRC)
```

**IRC shows only what was said.** Its tables also hold joins, parts and server
notices, and those are **45% of the 3.69M lines** (counted 2026-08-16: 33.5%
`event`, 11.8% `notice`) — so every read
restricts to `kind IN ('message', 'action')`; the conversation list gets that from
`irc_conversation_stats`, which signal's triggers only count those kinds into.
Excluded outright (`is_status`) is the conversation irssi files notices into: it
is named after your own nick, so it would show as a DM with yourself.

## Security model
Two layers, strongest first:
1. **Nextcloud login + allow-list (the real gate).** OAuth2 identity-only against
   `dash.xinutec.org` (copied from `life`). A successfully-authenticated user is
   still rejected (403) unless their NC id is in `ALLOWED_USERS` (currently
   `pippijn`). This holds regardless of network path.
2. **VPN-only by DNS.** `messages.xinutec.org` → `10.100.0.2` (isis's WireGuard
   IP), so it isn't listed on the public internet. NB this is *obscurity*: the
   isis ingress also answers on the public IP, so DNS alone doesn't firewall it
   — hence the login carries the security.
A third layer was considered and not built: an ingress
`whitelist-source-range: "10.100.0.0/24"` would make VPN-only real rather than
by-DNS, but only if client source IPs survive k3s servicelb — klipper may SNAT
them, so check before trusting it.

## Components
- `src/` — Rust/axum backend. `nextcloud/identity.rs` + `session.rs` are the
  `life` auth pattern verbatim; `routes/auth.rs` adds the allow-list check;
  `archive.rs` is the read-only, origin-normalising query layer; `irc_send.rs` is
  the one path that writes; `config.rs` builds the DB DSN from `DB_*` so it reuses
  `signal-secret` in-namespace. The only table this app owns is `sessions`
  (created on boot, `src/db.rs`).
- `frontend/` — Angular (login gate → conversation list with origin filter →
  thread view with reactions / edited / deleted markers, and a composer on IRC).
  Selecting across two or more messages and copying gives an irssi-style log —
  `src/app/copy-log.ts`, the inverse of `signal/src/irclog.rs`, actions included.
  `src/app/thread-window.ts` is the scrolling: it collapses all but a window of a
  long thread out of the DOM and re-anchors the viewport around every change.
  `src/app/attachment.ts` decides what an attachment is CALLED, because the
  screen and the clipboard both print it and drifted while each decided alone.
  A **deleted message shows `(deleted)` and nothing else until you click it** —
  its words AND its pictures. The archive keeps both (65 of 68 deleted rows still
  hold their text; 17 stored images hang off 3 of them), and the API sends them
  unredacted, so this is the reader deciding what to put on screen rather than
  anything being recovered. Until 2026-09-03 only the words were hidden and the
  images drew in full. A revealed message still copies as `(deleted)`: the log is
  built from the model, which the reveal does not touch.
  `src/app/generated/` is written by ts-rs from the Rust wire types
  (`scripts/gen-types.sh`) and imported through `src/app/models.ts`; don't
  hand-edit either.
- `Dockerfile` — multi-stage (Angular + Rust → one image), `xinutec/messages:latest`.
- The k8s manifests are **not here** — they live in the home monorepo, see Deploy.

## API (all require a valid session)
- `GET /api/me` — current user.
- `GET /api/conversations` — all origins, each tagged `origin`, newest first.
- `GET /api/conversations/{origin}/{id}/messages?cursor=<opaque>&limit=` — one
  page, oldest→newest, reactions attached; pass a previous page's `next_cursor`
  as `cursor` to page backwards (the cursor carries `(native_ts, id)`, so paging
  never skips messages that share a timestamp).
- `GET /api/search?q=` — substring search across all origins.
- `POST /api/conversations/irc/{id}/send` — say something, through irssi. Other
  origins 404: a decision, not a gap (`routes/api.rs`). The body is the text and
  nothing else — the recipient comes from `{id}`, so a request cannot address
  anyone the archive has not seen.
- `POST /api/telemetry` — fold client events into the server log. Always 204.

`{origin}` is `signal`, `gchat` or `irc`; `{id}` is the Signal `thread_id`, the
gchat `group_id`, or the numeric `irc_conversations.id`. That, a conversation's
`kind` (`dm`/`group`) and a message's (`message`/`action`) are all Rust enums, so
they reach the frontend as string unions rather than `string`, and an unknown
`{origin}` is a 404. A message's `kind` is IRC's only — the star on an action is
drawn by the client, not carried in the text.

## Local dev
```
# backend (needs the DB; tunnel signal-db or point at a local MariaDB)
DB_HOST=127.0.0.1 DB_PORT=3306 DB_NAME=signal DB_USER=… DB_PASSWORD=… \
NC_BASE_URL=https://dash.xinutec.org NC_CLIENT_ID=… NC_CLIENT_SECRET=… \
NC_REDIRECT_URI=http://localhost:4200/auth/callback \
SESSION_SECRET=$(openssl rand -hex 32) ALLOWED_USERS=pippijn \
  cargo run
# frontend (proxies /api,/login,/auth,/logout to :8080)
cd frontend && pnpm install && pnpm start  # http://localhost:4200
```

## Deploy (isis, namespace `signal`)
The manifests are in the home monorepo (`xinutec/pippijn`
`code/kubes/messages/k8s/`); run these from that checkout.

**Every time:** push to main, CI builds `xinutec/messages:latest`, then
`kubectl apply -f k8s/03-app.yaml -f k8s/04-ingress.yaml` and
`kubectl -n signal rollout status deploy/messages`.

**Done once, written down in case it must be redone:** register the OAuth2 client
in Nextcloud admin (redirect URI `https://messages.xinutec.org/auth/callback`);
put a Cloudflare `Zone:DNS:Edit` token in `cert-manager` as
`cloudflare-api-token` and apply `00-letsencrypt-dns-issuer.yaml`, because isis
has HTTP-01 only and this cert needs DNS-01; `messages → 10.100.0.2` is already
in `code/dns`; `NC_CLIENT_ID=… NC_CLIENT_SECRET=… ./k8s/secret.sh` writes the
session key and OAuth client (DB creds come from `signal-secret`).

## Tests
`gate.dhall` is the whole gate (and the pre-commit hook), twelve named checks:
fmt, clippy, generated-type drift, the Rust suite against a throwaway MariaDB,
then the frontend's deps + lint + e2e typecheck + build + unit tests + the
browser suite, and the shared dev-lint rules. Run it with

```sh
nix run ../dev-lint#gate -- . gate.json
```

`gate.json` is rendered from the Dhall and committed, so running the gate needs
no `dhall`; one of the checks re-renders and diffs the two.

- **Backend** `tests/access.rs` — the allow-list, which is the gate the security
  model above actually rests on (layer 2 is obscurity by its own admission). It
  had no test until 2026-09-03. What it pins is FAIL-CLOSED: every way of ending
  up with nobody on the list rejects everybody, including a blank or
  comma-only `ALLOWED_USERS`. Ablated two ways — dropping the parser's
  empty-entry filter, and reading "no list configured" as "no restriction" —
  and each fails it.
- **Backend** `tests/api_routes.rs` — the API through the real router, via
  `tower`'s `oneshot`. Everything else tests the decisions a handler makes; this
  tests that a request reaches it and that the auth extractor sits in front of
  every route needing one. ⚠ It touches no archive table on purpose: those are
  DROPped and recreated by `tests/archive.rs`, and cargo runs test binaries in
  parallel against the one database. Every case is decided before the archive is
  reached, which is the auth-and-routing surface and exactly what was untested.
- **Backend** `tests/session_cookie.rs`, `tests/error_responses.rs` — the cookie
  signature that makes a session unforgeable (tampered value, tampered or
  truncated signature, a cookie signed by another secret), and the rule that a
  500 describes nothing about itself. Both are about REJECTING, not the happy
  path. Ablated three ways — `find` for `rfind`, dropping the MAC check, putting
  the error in the body — and each fails only its own tests.
- **Backend** `tests/archive.rs` — pure units always run; the end-to-end ones seed
  a fixture into a throwaway MariaDB and assert the real queries. They need
  `MESSAGES_TEST_DATABASE_URL` and skip without it, so **running `cargo test` bare
  proves less than it looks like it does** — the gate's `tests` row starts an
  ephemeral MariaDB via `dev-lint`'s `with-test-db`.
- ⚠ **The composer is where jsdom lies to you.** A unit test for the IME guard
  passed against a version that still sent the half-composed word: calling the
  keydown handler in isolation never involves the `<form>`, and implicit
  submission reached `send` by a route the guard did not cover. Composition,
  implicit submission and the Android keyboard's viewport are browser behaviour;
  `e2e/ui-pages.spec.ts` drives a real composition through CDP and a real
  keyboard-sized viewport, and both were ablated. Do not accept a green vitest
  run as evidence about the composer.
- **Frontend** vitest (`pnpm test`): `app.spec.ts` shell, `thread.spec.ts` paging
  and composer, `copy-log.spec.ts` the clipboard format, `thread-window.spec.ts`
  the windowing arithmetic, `messages-store.spec.ts` shared state. ⚠ The window
  engine's MEASURING half is not there and should not be: jsdom has no layout, so
  every rect would be zero and the test would be of a fake. That half is
  `e2e/thread-scroll.spec.ts`, in a real browser. Then Playwright, split by whether jsdom could have answered:
  - `pnpm run ui-check` — **the whole browser suite, in the gate**: phone-width
    layout, the copy specs, the Android-keyboard and IME specs, and the
    scroll/routing/smoke behaviour specs. 31 tests, ~6s.
  - ⚠ There was a second config for scroll/routing/smoke until 2026-09-03,
    because they were written against `ng serve` and one was believed to need
    it. Re-tested rather than inherited: they pass against the production build,
    45/45 on repeat. The cost of that split was that the windowing engine — the
    subtlest code here — had its only browser coverage in the suite nobody
    ran.

## One concept, several readers

Every field below is interpreted in more than one place, and each place can
forget the rule independently. That is not hypothetical: **three of the four
defects found on 2026-09-03 were exactly this**, and the last of them was found
by writing this table rather than by anyone hitting it. Add a row when a field
gains a second reader.

| field | Rust | thread.html | copy-log.ts | search |
| --- | --- | --- | --- | --- |
| `deleted` | Signal reads it; gchat/IRC are always `false` | hidden behind a click, body AND attachments | `(deleted)` and nothing else, attachments included | the hit matches and is listed, with `(deleted)` where the snippet goes |
| `edited` | Signal only | `edited` tag in the meta line | ` (edited)` on the last line | not shown |
| `kind` | IRC only; two of the column's four values | `* ` before the body | `HH:MM  * nick ` prefix | not shown |
| `is_outgoing` | all three origins | `.out` class | nothing — the sender's name carries it | not shown |

⚠ **One known divergence, deliberate.** A message with no body and no
attachments (20 of them) draws an empty bubble but produces no line in a copied
log; an empty log line would say less than nothing.

**Deleted content: one policy, decided 2026-09-04.** Search excluded `deleted`
rows in SQL while a thread sent the same text and hid it behind a click — two
policies for one concept, chosen in two places, neither aware of the other. It
is now the thread's, everywhere: **the server sends retracted text, and every
reader hides it until asked**. A search that cannot find what was retracted is
not an archive's search, so a hit is listed with `(deleted)` where its snippet
would go — you learn a retraction matched, in which conversation and when,
without being handed the words. Clicking through reaches the thread's reveal.

Withholding the text server-side was the alternative and was rejected on cost,
not on principle: searching and returning are separable (the `LIKE` runs in
MySQL either way), so the hit could carry no text at all and a per-message
endpoint could serve the reveal. It buys only that devtools cannot see a
retraction on a screen its authenticated owner is already looking at, and costs
an endpoint, a round trip and a second loading state. Revisit if this archive
ever has a reader who is not its owner.

## Known limits
- The Signal reaction count approximates live state as distinct non-removed
  authors per emoji, so a same-author add-then-remove inside one page is missed.
- Signal edit history is flat: an edit shows as edited, not as a chain.
- Attachments are Signal-only (the Google Chat export carries none, IRC has no
  such thing), served from the PVC mounted read-only. Metadata-only history rows
  are shown but marked not stored.
- **A large attachment is read whole into memory** (`tokio::fs::read`), against
  the pod's 256Mi limit. Not a risk at today's sizes — 591 stored files, largest
  22.6 MB, mean 687 KB, measured 2026-09-03 — but the mechanism is unbounded
  while the ceiling is not, so re-measure rather than assume if Signal's cap or
  the limit moves. Streaming would remove the coupling.
- **Copying reaches only what is rendered — and now says so.** The thread keeps a
  bounded window in the DOM (400 messages), so a select-all in a long
  conversation copies that window rather than the conversation. The limit is
  unchanged; what changed on 2026-09-03 is that such a copy ends with
  `--- copied 400 of 401794 messages; the rest were not loaded on screen`. The
  notice rides in the text because a warning in the app is not there when the
  paste is read somewhere else, and it appears only when the selection took the
  WHOLE window — a deliberate two-line quote is not a truncated copy.
