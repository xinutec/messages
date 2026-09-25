{-
messages/gate.dhall — this repository's commit gate.

The generated `gate.json` is committed, and `the table matches its Dhall`
re-renders and diffs it, so running the gate needs no `dhall`.
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
            fingerprint differently and would evict each other's cache.
        -}
        G.Check::{
        , name = "clippy"
        , argv =
            G.inDevShell
              [ "cargo"
              , "clippy"
              , "--workspace"
              , "--all-targets"
              , "--"
              , "-D"
              , "warnings"
              ]
        , env =
            G.clippyTarget
        , timeout_s = 1800
        }
      , G.cargoDoc
      , {-  Fails if the committed frontend types no longer match the Rust ones.
        -}
        G.Check::{
        , name = "generated types are current"
        , argv = G.inDevShell [ "scripts/gen-types.sh", "--check" ]
        , timeout_s = 900
        }
      , {-  The whole Rust suite against a throwaway MariaDB, as CI does. The port
            must be unique across the fleet's gates, which run concurrently.
        -}
        G.Check::{
        , name = "tests (against a real MariaDB)"
        , argv =
            G.withTestDb
              "../"
              [ "--database"
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
      , {-  The archiver's suite, in its own database: it applies the real
            migrations, which the viewer's fixture would collide with.
        -}
        G.Check::{
        , name = "archiver tests (against a real MariaDB)"
        , argv =
            G.withTestDb
              "../"
              [ "--database"
              , "signal_test"
              , "--user"
              , "signal"
              , "--password"
              , "signal"
              , "--port"
              , "3322"
              , "--url-env"
              , "SIGNAL_TEST_DATABASE_URL"
              , "--"
              , "cargo"
              , "test"
              , "-p"
              , "signal-archiver"
              ]
        , timeout_s = 1800
        }
      , {-  Installs exactly pnpm-lock.yaml, so the gate works from a clean checkout.
        -}
        G.Check::{
        , name = "frontend deps match the lockfile"
        , cwd = "frontend"
        , argv = G.inDevShell [ "pnpm", "install", "--frozen-lockfile" ]
        , env = G.nonInteractive
        , timeout_s = 900
        }
      , G.Check::{
        , name = "frontend lint"
        , cwd = "frontend"
        , argv = G.inDevShell [ "pnpm", "run", "lint" ]
        , env = G.nonInteractive
        , timeout_s = 900
        }
      , G.Check::{
        , name = "frontend typecheck (e2e)"
        , cwd = "frontend"
        , argv = G.inDevShell [ "pnpm", "run", "typecheck:e2e" ]
        , env = G.nonInteractive
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
        , env = G.nonInteractive
        , timeout_s = 1800
        }
      , G.Check::{
        , name = "frontend unit tests"
        , cwd = "frontend"
        , argv = G.inDevShell [ "pnpm", "test" ]
        , env = G.nonInteractive # G.oneAngularWorker
        , timeout_s = 1800
        }
      , {-  Everything that needs a real browser against the production build:
            phone-width layout, copy (jsdom's Selection API is wrong), scroll,
            routing and smoke.
        -}
        G.Check::{
        , name = "frontend browser suite (layout, copy, scroll, routing, smoke)"
        , cwd = "frontend"
        , argv = G.inDevShell [ "pnpm", "run", "ui-check" ]
        , {-  Playwright deletes this at the start of every run, so the gate copies it
              aside when this check fails.
          -}
          artifacts = [ "test-results" ]
        , env = G.nonInteractive
        , timeout_s = 1800
        }
      , G.checkTable "../dev-lint"
      , G.devLint "../"
      ]
    }
