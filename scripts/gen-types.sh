#!/usr/bin/env bash
# Generate the frontend TS interfaces from the Rust API types via ts-rs, so the
# backend↔frontend wire shapes are consistent by construction (not transcribed).
#
# Run inside the messages dev shell (cargo on PATH):
#   nix develop --command scripts/gen-types.sh
#
# Output lands in frontend/src/app/generated/ (committed; imported via
# frontend/src/app/models.ts). The drift gate re-runs this and fails if the
# committed output no longer matches the Rust types — see scripts/check-types.sh.
set -euo pipefail
cd "$(dirname "$0")/.."

OUT="frontend/src/app/generated"

# Generate FIRST, into a scratch dir, and only replace the committed output once
# it has actually worked — a generator that fails must leave the previous output
# exactly where it was, rather than deleting the frontend's types and reporting
# nothing. (Learned in coach, where the reverse order did exactly that.)
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# ts-rs emits one file per #[ts(export)] type; the export tests are named
# export_bindings_*, so this filter runs only generation — tests/archive.rs's
# end-to-end cases need MariaDB and are not selected. The output dir is pinned in
# .cargo/config.toml (TS_RS_EXPORT_DIR), overridden here so a failed run can't
# touch the committed types.
#
# --features ts turns on ts-rs (off by default: normal builds carry none).
if ! TS_RS_EXPORT_DIR="$TMP" cargo test --features ts export_bindings >"$TMP/cargo.log" 2>&1; then
  echo "gen-types: generation failed — committed types left untouched." >&2
  # The compile errors are the whole point of running this; show them.
  grep -E '^(error|warning: unused)|^ *-->' "$TMP/cargo.log" >&2 || tail -30 "$TMP/cargo.log" >&2
  exit 1
fi

count="$(find "$TMP" -name '*.ts' | wc -l | tr -d ' ')"
if [ "$count" -eq 0 ]; then
  echo "gen-types: generation produced no types — committed types left untouched." >&2
  exit 1
fi

rm -rf "$OUT"
mkdir -p "$(dirname "$OUT")"
cp -R "$TMP" "$OUT"
rm -f "$OUT/cargo.log"
echo "generated $count type(s) -> $OUT"
