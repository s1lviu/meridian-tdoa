#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
output=${1:?Usage: ./scripts/backup.sh /path/to/backup.tar.gz}
umask 077
# Stop the worker to obtain a consistent capture+manifest+results snapshot.
docker compose stop app
trap 'docker compose start app >/dev/null' EXIT HUP INT TERM
docker compose run --rm --no-deps --entrypoint tar app -C /app -czf - data > "$output"
tar -tzf "$output" >/dev/null
printf '%s\n' "Backup verified: $output"
