#!/usr/bin/env bash
# S1 determinism harness: native vs wasm32 state hashes must be
# bit-identical for every golden circuit. Run from repo root.
set -euo pipefail

out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT

cargo run --release -p sim-golden --bin hash > "$out/native.txt"

# NOT >/dev/null: a wasm-pack failure used to vanish into it, and `set -e`
# then killed the script with a bare exit 1 and no explanation — which reads
# exactly like "determinism broke". It cost real time to rediscover that the
# actual cause was the pinned toolchain missing from PATH. A harness whose
# failure mode is indistinguishable from the thing it guards against is worse
# than no harness.
if ! wasm-pack build crates/sim-wasm --release --target nodejs \
  --out-dir ../../target/wasm-node -- --features golden > "$out/wasm-build.log" 2>&1; then
  echo "WASM BUILD FAILED — this is a BUILD problem, not a determinism result:" >&2
  tail -30 "$out/wasm-build.log" >&2
  echo "(is the pinned toolchain on PATH? rust-toolchain.toml pins it)" >&2
  exit 2
fi

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
