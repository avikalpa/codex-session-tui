#!/usr/bin/env bash
set -euo pipefail

IN_FILE="${1:-demo.cast}"
OUT_FILE="${2:-assets/demo.svg}"

if [[ ! -f "$IN_FILE" ]]; then
  echo "Input cast not found: $IN_FILE" >&2
  exit 1
fi

mkdir -p "$(dirname "$OUT_FILE")"

# Large casts can be memory-heavy for svg-term-cli; raise Node heap.
NODE_OPTIONS="${NODE_OPTIONS:---max-old-space-size=8192}" \
  npx -y svg-term-cli --in "$IN_FILE" --out "$OUT_FILE" --window --padding 8

echo "Wrote $OUT_FILE"
