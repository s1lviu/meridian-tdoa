#!/bin/sh
set -eu
cd /app
mkdir -p data/results data/captures data/datasets data/jobs data/state
if [ ! -f data/recordings/manifest.json ]; then cp -R /opt/meridian-seed/recordings data/; fi
for source in /opt/meridian-seed/results/*.json; do
    name=$(basename "$source")
    if [ ! -f "data/results/$name" ]; then cp "$source" "data/results/$name"; fi
done
exec "$@"
