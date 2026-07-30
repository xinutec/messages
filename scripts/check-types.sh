#!/usr/bin/env bash
# Drift gate: regenerate the TS types and fail if the committed output changed.
# Catches a Rust API-type edit that wasn't regenerated + committed. Run in the
# dev shell (cargo on PATH); part of scripts/verify.sh — i.e. the pre-commit gate.
set -euo pipefail
cd "$(dirname "$0")/.."

# Snapshot first, then compare the regenerated output against THAT — not against
# the index. `git diff` answers "are these staged?", which is a different
# question and gives a false drift whenever the types are correctly regenerated
# but not yet `git add`ed, i.e. the normal edit→verify→commit order. The message
# then blames the one thing that isn't wrong.
before="$(mktemp -d)"
trap 'rm -rf "$before"' EXIT
cp -R frontend/src/app/generated/. "$before"/

scripts/gen-types.sh >/dev/null

if ! diff -r -q "$before" frontend/src/app/generated >/dev/null 2>&1; then
  echo "gen-types drift: the Rust API types changed but frontend/src/app/generated/" >&2
  echo "was not regenerated. Run 'nix develop --command scripts/gen-types.sh' and commit." >&2
  diff -r -q "$before" frontend/src/app/generated >&2 || true
  exit 1
fi
echo "types in sync."
