{-
messages/gate.dhall — this repository's commit gate.

Was `scripts/verify.sh` plus `scripts/with-test-db.sh`. That second file opened
with "Ported from fleetwatch, socket caveat and all", and this is the other half
of undoing that: the ephemeral MariaDB is `dev-lint`'s `with-test-db` now, one
implementation for the three repositories that were carrying near-identical
copies. The five values that differed between them — the database, the
credentials, the port, the env var, the temp-dir prefix — are the row below,
typed and visible rather than buried in a copied script.

**What the DB row is actually for**, kept from the script's own comment because
it is the reason the row exists: until 2026-07-30 this gate ran no `cargo test`
at all. Clippy compiled the tests and nothing ran them, so the pre-commit gate
could pass on a commit that broke every query in the app, and CI was the first
thing to notice. A row that names itself is harder to lose than a line inside a
`&&` chain.

**The build is checked rather than hoped for.** The script set
`NG_BUILD_MAX_WORKERS=1` and said a spurious abort "is worked around by
re-running verify" — so a complete, valid bundle that hit the macOS Piscina
teardown abort failed the gate and cost a manual re-run, and nothing ever
asserted what the build had produced. `ng-build` decides from the artifact:
index.html present, non-empty, rewritten by this run, and every script it names
parseable as an ES module.

**The conditional `pnpm install` is gone**, for the reason gamepads', coach's,
memview's and fleetwatch's were: its own comment justified it on correctness — a
node_modules left behind by npm still has a working `.bin`, so verify would pass
against packages the lockfile no longer describes — and running it
unconditionally serves that better. Measured on gamepads before cutting: an
up-to-date `--frozen-lockfile` install is 455 ms.

**The `&&` chain is gone.** `pnpm run lint && pnpm run typecheck:e2e && pnpm exec
ng build && pnpm test && pnpm run ui-check` reported one name when five things
could be wrong.

The generated `gate.json` is committed; `the table matches its Dhall` re-renders
and diffs it, so running the gate needs no `dhall`.

**The vocabulary moved into the schema.** `inDevShell`, the clippy target
directory, the Angular worker cap, and the `ng-build` / `dev-lint` /
`check-table` rows were spelled out here and in a dozen other tables
identically — the duplication the shared tools were built to remove, recreated
one level up. They are `G.` values now. Two consequences the rendered JSON
shows: every dev-shell row gains `--no-warn-dirty`, because a gate that prints
"Git tree is dirty" on every row of every run has trained everyone to ignore a
warning; and dev-lint is pinned to its committed HEAD rather than run out of its
worktree, which is what stops a neighbour's half-finished edit failing this gate
for a reason no commit anywhere explains.

-}

let G = ../dev-lint/gate/schema.dhall

in  { name = "messages"
    , checks =
      [ G.Check::{
        , name = "formatting"
        , argv = G.inDevShell [ "cargo", "fmt", "--all", "--check" ]
        , timeout_s = 180
        }
      , {-  Clippy gets its own target directory: clippy-driver and rustc
            fingerprint the workspace differently and evict each other in a
            shared one, forcing a full recompile.

            The script read this from `$CARGO_CLIPPY_TARGET_DIR` with the path
            below as the default. A table's `env` is data, not shell, so there is
            no expansion — and the override had no other caller, so the default
            is simply the value now.
        -}
        G.Check::{
        , name = "clippy"
        , argv =
            G.inDevShell
              [ "cargo", "clippy", "--all-targets", "--", "-D", "warnings" ]
        , env =
            G.clippyTarget
        , timeout_s = 1800
        }
      , {-  The frontend types are generated from the Rust wire types; fail if the
            committed output no longer matches them. Placed before the frontend
            rows, since a drifted type is what the build would then compile
            against — though placement is presentation only, and it would run
            regardless.
        -}
        G.Check::{
        , name = "generated types are current"
        , argv = G.inDevShell [ "scripts/check-types.sh" ]
        , timeout_s = 900
        }
      , {-  The whole Rust suite, the end-to-end tests in tests/archive.rs
            included, against a throwaway MariaDB — the same
            MESSAGES_TEST_DATABASE_URL that CI sets from its MariaDB service.

            No `--grant-all`: this suite uses the one database it is given, and
            the narrow default is what stops it inheriting rights it never asked
            for. Port 3318 — fleetwatch's ephemeral server takes 3317 and coach's
            3319, so the fleet gate can run all three at once.
        -}
        G.Check::{
        , name = "tests (against a real MariaDB)"
        , argv =
              G.inDevShell [ "nix", "run", "../dev-lint#with-test-db", "--" ]
            # [ "--database"
              , "messages_test"
              , "--user"
              , "messages"
              , "--password"
              , "messages"
              , "--port"
              , "3318"
              , "--url-env"
              , "MESSAGES_TEST_DATABASE_URL"
              , "--"
              , "cargo"
              , "test"
              ]
        , timeout_s = 1800
        }
      , {-  `--frozen-lockfile` is pnpm ci: install exactly pnpm-lock.yaml, or
            fail. The gate has to run from a clean checkout — a fresh clone, or
            the tree the fleetwatch collector runs in — not just a warm dev
            machine.
        -}
        G.Check::{
        , name = "frontend deps match the lockfile"
        , cwd = "frontend"
        , argv = G.inDevShell [ "pnpm", "install", "--frozen-lockfile" ]
        , timeout_s = 900
        }
      , G.Check::{
        , name = "frontend lint"
        , cwd = "frontend"
        , argv = G.inDevShell [ "pnpm", "run", "lint" ]
        , timeout_s = 900
        }
      , G.Check::{
        , name = "frontend typecheck (e2e)"
        , cwd = "frontend"
        , argv = G.inDevShell [ "pnpm", "run", "typecheck:e2e" ]
        , timeout_s = 900
        }
      , {-  `../../dev-lint`, not `../dev-lint`: cwd is `messages/frontend`.
        -}
        G.Check::{
        , name = "frontend build"
        , cwd = "frontend"
        , argv =
            G.ngBuild
              "../../"
              [ "dist/messages-web/browser" ]
              [ "pnpm", "exec", "ng", "build" ]
        , timeout_s = 1800
        }
      , G.Check::{
        , name = "frontend unit tests"
        , cwd = "frontend"
        , argv = G.inDevShell [ "pnpm", "test" ]
        , env = G.oneAngularWorker
        , timeout_s = 1800
        }
      , {-  The L2 phone-width layout harness: `e2e/serve.mjs` serves the dist the
            build row wrote and the specs assert no overlap or overflow at Pixel
            width.
        -}
        G.Check::{
        , name = "frontend ui-check (phone-width layout harness)"
        , cwd = "frontend"
        , argv = G.inDevShell [ "pnpm", "run", "ui-check" ]
        , timeout_s = 1800
        }
      , G.checkTable "../dev-lint"
      , G.devLint "../"
      ]
    }
