#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --locked
if [ ! -f data/results/dcf77-20200813.json ]; then
  target/debug/meridian-tdoa process data/recordings/manifest.json > /dev/null
fi
exec target/debug/meridian-tdoa
