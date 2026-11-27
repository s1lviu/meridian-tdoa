#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p evidence
python3 scripts/prepare_recording.py
cargo build --locked
target/debug/meridian-tdoa process data/recordings/manifest.json > evidence/replay-result.json
target/debug/meridian-tdoa process data/recordings/manifest.json pa0rdt > evidence/sync-loss-result.json
python3 scripts/evaluate.py
