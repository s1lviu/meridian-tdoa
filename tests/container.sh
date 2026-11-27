#!/bin/sh
# Offline Linux image readiness and SQLite persistence through container replacement.
set -eu
image=${1:?Pass the local Meridian image tag}
volume=$(docker volume create)
instance=
cleanup() {
    if [ -n "$instance" ]; then docker rm -f "$instance" >/dev/null; fi
    docker volume rm "$volume" >/dev/null
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
start() {
    instance=$(docker run -d --read-only --cap-drop=ALL --network none --tmpfs /tmp \
        -v "$volume:/app/data" -e MERIDIAN_CONFIG=/app/config/app.json \
        -e MERIDIAN_DIRECTORY_OFFLINE=1 "$image")
    docker exec -i "$instance" python3 - <<'PYTHON'
import json
import time
from urllib.request import urlopen

base = 'http://127.0.0.1:3000'
for _ in range(120):
    try:
        with urlopen(base + '/api/ready', timeout=1) as response:
            assert response.status == 200
        break
    except OSError:
        time.sleep(0.25)
else:
    raise SystemExit('Meridian did not become ready')

with urlopen(base + '/api/emissions/dcf77-20200813', timeout=3) as response:
    replay = json.load(response)
    assert replay['mode'] == 'replay' and len(replay['hypotheses']) == 2
PYTHON
}
migration_time() {
    docker exec -i "$instance" python3 - <<'PYTHON'
import sqlite3
with sqlite3.connect('file:/app/data/meridian.sqlite?mode=ro', uri=True) as db:
    assert db.execute('pragma integrity_check').fetchone() == ('ok',)
    print(db.execute("select value from meta where key='legacy_json_import'").fetchone()[0])
PYTHON
}
start
original=$(migration_time)
docker rm -f "$instance" >/dev/null
instance=
start
test "$original" = "$(migration_time)"
echo "Linux image passed readiness, replay and persistent-volume checks."
