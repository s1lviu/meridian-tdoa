#!/bin/sh
set -eu
cd /app
mkdir -p data/results data/captures data/datasets data/jobs data/state
# The map shows live data only. The archived 2020 DCF77 recording stays in the image as a test
# fixture and is copied into data/ only when MERIDIAN_SEED_RECORDING=1 (tests/container.sh).
if [ "${MERIDIAN_SEED_RECORDING:-0}" = 1 ]; then
    if [ ! -f data/recordings/manifest.json ]; then cp -R /opt/meridian-seed/recordings data/; fi
    for source in /opt/meridian-seed/results/*.json; do
        name=$(basename "$source")
        if [ ! -f "data/results/$name" ]; then cp "$source" "data/results/$name"; fi
    done
fi
exec "$@"
