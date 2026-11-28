#!/bin/sh
# Offline Linux image readiness and SQLite persistence through container replacement.
# The archived DCF77 recording is seeded explicitly: production images start with live data only.
set -eu
image=${1:?Pass the local Meridian image tag}
MERIDIAN_ADMIN_TOKEN=$(openssl rand -hex 32)
export MERIDIAN_ADMIN_TOKEN
volume=$(docker volume create)
instance=
cleanup() {
    if [ -n "$instance" ]; then docker rm -f "$instance" >/dev/null; fi
    docker volume rm "$volume" >/dev/null
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
start() {
    instance=$(docker run -d --read-only --cap-drop=ALL --security-opt=no-new-privileges \
        --memory=1g --pids-limit=128 --network none --tmpfs /tmp:size=128m,mode=1777 \
        -v "$volume:/app/data" -e MERIDIAN_ADMIN_TOKEN -e MERIDIAN_SEED_RECORDING=1 \
        -e MERIDIAN_DIRECTORY_OFFLINE=1 "$image")
    docker exec -i "$instance" python3 - <<'PYTHON'
import json
import os
import time
from pathlib import Path
from urllib.request import urlopen

base = 'http://127.0.0.1:3000'
assert os.getuid() == 10001
assert os.statvfs('/').f_flag & os.ST_RDONLY
for maximum, paths in [
    (1073741824, ['memory.max', 'memory/memory.limit_in_bytes']),
    (128, ['pids.max', 'pids/pids.max']),
]:
    limit = next(p for name in paths if (p := Path('/sys/fs/cgroup') / name).is_file())
    actual = limit.read_text().strip()
    assert actual.isdecimal() and 0 < int(actual) <= maximum, (limit, actual)
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
with urlopen(base + '/api/info', timeout=3) as response:
    assert json.load(response)['live_enabled'] is True
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
replay_job=$(docker exec -i "$instance" python3 - <<'PYTHON'
import json
import os
import time
from urllib.request import Request, urlopen

base = 'http://127.0.0.1:3000'
request = Request(base + '/api/replay',
                  data=json.dumps({'dataset_id': 'dcf77-20200813', 'force': True}).encode(),
                  headers={'Authorization': 'Bearer ' + os.environ['MERIDIAN_ADMIN_TOKEN'],
                           'Content-Type': 'application/json'})
with urlopen(request, timeout=5) as response:
    submitted = json.load(response)
    assert not submitted.get('cached')
    job_id = submitted['job_id']
for _ in range(300):
    with urlopen(base + '/api/jobs/' + job_id, timeout=3) as response:
        job = json.load(response)
    if job['status'] != 'running':
        assert (job['status'], job.get('emission_id')) == ('completed', 'dcf77-20200813'), job
        print(job_id)
        break
    time.sleep(0.5)
else:
    raise SystemExit('Reprocessing the bundled IQ timed out')
PYTHON
)
docker rm -f "$instance" >/dev/null
instance=
start
test "$original" = "$(migration_time)"
docker exec "$instance" python3 -c '
import json, sys, urllib.request
with urllib.request.urlopen("http://127.0.0.1:3000/api/jobs/" + sys.argv[1], timeout=3) as response:
    job = json.load(response)
    assert job["id"] == sys.argv[1] and job["status"] == "completed"
' "$replay_job"
echo "Linux image passed readiness, resource limits, forced IQ replay and persistent-volume checks."
