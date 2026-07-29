#!/usr/bin/env bash
# S1 determinism harness: native vs wasm32 state hashes must be
# bit-identical for every golden circuit. Run from repo root.
set -euo pipefail

out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT

cargo run --release -p sim-golden --bin hash > "$out/native.txt"

wasm-pack build crates/sim-wasm --release --target nodejs \
  --out-dir ../../target/wasm-node -- --features golden > /dev/null 2>&1

node -e "
const { goldenHash, goldenNames } = require(process.cwd() + '/target/wasm-node/sim_wasm.js');
for (const n of goldenNames()) {
  console.log(goldenHash(n, 10000));
}" > "$out/wasm.txt"

if diff "$out/native.txt" "$out/wasm.txt"; then
  echo "DETERMINISM OK: native ($(uname -m)) == wasm32"
  cat "$out/native.txt"
else
  echo "DETERMINISM BROKEN: hashes differ between native and wasm32" >&2
  exit 1
fi
