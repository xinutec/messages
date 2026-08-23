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
notices — the notices alone are 47% of the archive's lines — so every read
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
gchat `group_id`, or the numeric `irc_conversations.id`. Both it and a
conversation's `kind` are Rust enums, so they reach the frontend as string unions
rather than `string`, and an unknown `{origin}` is a 404.

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
then the frontend's deps + lint + e2e typecheck + build + unit tests +
phone-layout harness, and the shared dev-lint rules. Run it with

```sh
nix run ../dev-lint#gate -- . gate.json
```

`gate.json` is rendered from the Dhall and committed, so running the gate needs
no `dhall`; one of the checks re-renders and diffs the two.

- **Backend** `tests/archive.rs` — pure units always run; the end-to-end ones seed
  a fixture into a throwaway MariaDB and assert the real queries. They need
  `MESSAGES_TEST_DATABASE_URL` and skip without it, so **running `cargo test` bare
  proves less than it looks like it does** — the gate's `tests` row starts an
  ephemeral MariaDB via `dev-lint`'s `with-test-db`.
- **Frontend** vitest (`pnpm test`): `app.spec.ts` shell, `thread.spec.ts` paging
  and composer, `copy-log.spec.ts` the clipboard format, `messages-store.spec.ts`
  shared state. Playwright for the rest —
  `pnpm run ui-check` (phone-width layout, in the gate) and `pnpm run
  e2e:behaviour` (routing/scroll/copy, on demand). ⚠ The copy specs live there
  because jsdom gets the Selection API wrong — measured 2026-08-16: its
  `containsNode` called a bubble past the range selected, and the bubble a small
  selection sat inside unselected.

## Known limits
- The Signal reaction count approximates live state as distinct non-removed
  authors per emoji, so a same-author add-then-remove inside one page is missed.
- Signal edit history is flat: an edit shows as edited, not as a chain.
- Google Chat has no attachments — the export carries none.
- Attachments are Signal-only and served from the PVC, mounted read-only.
  Metadata-only history rows are shown but marked not stored.
