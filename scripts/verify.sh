#!/usr/bin/env bash
# messages verify — rust backend (fmt + clippy + the whole test suite, against an
# ephemeral MariaDB) + generated-type drift + angular frontend (lint + build +
# unit tests + layout harness) + shared rules. Toolchain comes from the flake devshell
# (rev-pinned via flake.lock), so it's reproducible without cargo/npm on PATH.
set -euo pipefail
cd "$(dirname "$0")/.."
nix develop -c bash -c '
  set -euo pipefail
  # @angular/build:application tears down its Piscina worker pool at process
  # exit; on macOS / Node 24 / libuv 1.52 that teardown intermittently aborts
  # the process — a libuv kqueue assertion ("errno == EINTR", uv__io_poll →
  # Abort 6) or "EBADF: bad file descriptor, close" — AFTER "bundle generation
  # complete". NG_BUILD_MAX_WORKERS=1 lowers the rate (fewer worker pipes to
  # race) but does NOT eliminate it; a spurious build abort here is worked
  # around by re-running verify. Harmless on Linux/CI, which build cleanly.
  export NG_BUILD_MAX_WORKERS=1
  cargo fmt --all --check
  # Clippy gets its own target dir: clippy-driver and rustc fingerprint the
  # workspace differently and evict each other in a shared dir, forcing a full
  # recompile. A dedicated dir keeps both caches warm.
  CARGO_TARGET_DIR="${CARGO_CLIPPY_TARGET_DIR:-$HOME/.cache/cargo/clippy-target}" \
    cargo clippy --all-targets -- -D warnings
  # The frontend types are generated from the Rust wire types; fail if the
  # committed output no longer matches them. Runs before the frontend build,
  # since a drifted type is what the build would then compile against.
  scripts/check-types.sh
  # The whole Rust suite, end-to-end DB tests included — with-test-db.sh brings
  # up a throwaway MariaDB and points MESSAGES_TEST_DATABASE_URL at it, the same
  # variable CI sets from its MariaDB service. Until 2026-07-30 this gate ran no
  # `cargo test` at all: clippy compiled the tests and nothing ran them, so the
  # pre-commit gate could pass on a commit that broke every query in the app, and
  # CI was the first thing to notice.
  scripts/with-test-db.sh cargo test
  # ui-check (L2 phone-width layout harness) runs after the build — it serves
  # the freshly-built dist via e2e/serve.mjs and asserts no overlap/overflow at
  # Pixel width. See @xinutec/ui-harness + dev-lint/docs/layout-quality-architecture.md.
  # Frontend deps must exist before lint/build. verify.sh has to run from a clean
  # checkout (a fresh clone, or the tree the fleetwatch collector runs in) — not
  # just a warm dev machine — so install them when absent or the lockfile moved.
  # --frozen-lockfile is pnpm ci: install exactly pnpm-lock.yaml, or fail. The
  # guard is not just a speed-up — a node_modules left behind by npm still has a
  # working .bin, so verify would pass against packages the lockfile no longer
  # describes.
  if [ ! -d frontend/node_modules ] || [ frontend/pnpm-lock.yaml -nt frontend/node_modules ]; then
    ( cd frontend && pnpm install --frozen-lockfile )
  fi
  ( cd frontend && pnpm run lint && pnpm exec ng build && pnpm test && pnpm run ui-check )
'
dev_lint_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/dev-lint"
[ -d "$dev_lint_dir" ] || dev_lint_dir="$HOME/Code/dev-lint"
[ -d "$dev_lint_dir" ] || dev_lint_dir="$HOME/code/dev-lint"
nix run "$dev_lint_dir" -- . # dev-lint
