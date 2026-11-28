#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --locked
binary="${CARGO_TARGET_DIR:-target}/debug/meridian-tdoa"
if [ ! -f data/results/dcf77-20200813.json ]; then
  "$binary" process data/recordings/manifest.json > /dev/null
fi
exec "$binary"
