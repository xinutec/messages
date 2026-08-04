#!/usr/bin/env bash
# Generate the frontend TS interfaces from the Rust types via ts-rs, so the
# backend↔frontend wire shapes are consistent by construction, not transcribed.
#
#   nix develop --command scripts/gen-types.sh            # regenerate + install
#   nix develop --command scripts/gen-types.sh --check    # report drift, write nothing
#
# The second form is what the gate's generated-types row runs, so the cargo
# invocation below is stated once and both paths use it.
#
# All this file holds is the part that is this repository's: where the bindings
# live and how to make cargo emit them. The rest — generate into a scratch
# directory and install only on success, refuse a generation that emitted
# nothing, copy the types and not whatever else landed beside them, compare by
# content rather than by asking git — is dev-lint#gen-types, shared with the
# four other repositories that had each grown their own version of it.
# scripts/check-types.sh is gone with it.
#
# `--features ts` turns on ts-rs (off by default: normal builds carry none). The
# export tests are named export_bindings_*, so the filter runs generation only —
# tests/archive.rs's end-to-end cases need MariaDB and are not selected.
set -euo pipefail
cd "$(dirname "$0")/.."

exec nix run ../dev-lint#gen-types -- "$@" \
  --out frontend/src/app/generated \
  -- cargo test --features ts export_bindings
