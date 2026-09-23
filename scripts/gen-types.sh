#!/usr/bin/env bash
# Generate the frontend's TS types from the Rust wire types via ts-rs.
#
#   nix develop --command scripts/gen-types.sh            # regenerate + install
#   nix develop --command scripts/gen-types.sh --check    # report drift (the gate)
#
# The generic part is dev-lint#gen-types; this names where the bindings go and
# how cargo emits them. `--features ts` turns ts-rs on; the export tests are
# named export_bindings_*, so the filter skips the database tests.
set -euo pipefail
cd "$(dirname "$0")/.."

# dev-lint's committed HEAD, not its working tree; see `withTestDb` in
# dev-lint/gate/schema.dhall.
exec nix run "git+file:../dev-lint?ref=HEAD#gen-types" -- "$@" \
  --out frontend/src/app/generated \
  -- cargo test --features ts export_bindings
