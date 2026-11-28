#!/bin/sh
# Build and start one release on a host prepared by deploy/host/setup.sh.
# Called by scripts/deploy-host.sh as the deploy user: sh activate.sh RELEASE
# On a failed health check the previous image is started again. Keeps the three newest releases.
set -eu
release=${1:?release id}
root=/opt/meridian
image=meridian-tdoa:$release
cd "$root"
[ -s .secrets/runtime.env ] && [ -s .env ] || { echo "Run deploy/host/setup.sh first." >&2; exit 1; }
mountpoint -q /srv/meridian/data || { echo "/srv/meridian/data is not mounted." >&2; exit 1; }
port=$(sed -n 's/^MERIDIAN_PORT=//p' .env)

# Isolated builder: compiling uses at most three cores and 6 GiB, its cache stays under 8 GB,
# and pruning it never touches the build caches of other projects on this host.
if ! docker buildx inspect meridian >/dev/null 2>&1; then
    docker buildx create --name meridian --driver docker-container \
        --driver-opt cpuset-cpus=0-2 --driver-opt memory=6g --driver-opt memory-swap=6g >/dev/null
fi
docker buildx build --builder meridian --load --tag "$image" "releases/$release"
docker buildx prune --builder meridian --max-used-space 8gb --force >/dev/null
# The builder container is idle between deploys; its cache survives the stop.
docker buildx stop meridian

cp "releases/$release/deploy/host/compose.yaml" compose.yaml
[ -f image.env ] && cp image.env image.previous.env
printf 'MERIDIAN_IMAGE=%s\n' "$image" > image.env
compose() { docker compose --project-directory "$root" --env-file .env --env-file image.env "$@"; }
if compose up -d --wait --wait-timeout 180 &&
    curl -fsS --max-time 10 "http://127.0.0.1:$port/api/ready" >/dev/null; then
    echo "Release $release is live on 127.0.0.1:$port"
else
    echo "Release $release failed its health check." >&2
    compose logs --tail 40 app >&2 || true
    if [ -f image.previous.env ]; then
        cp image.previous.env image.env
        compose up -d --wait --wait-timeout 180 && echo "Rolled back to $(cat image.env)" >&2
    fi
    exit 1
fi

# Keep the three newest releases and their images; older ones only take disk space.
ls -1t releases | tail -n +4 | while read -r old; do
    rm -rf "releases/$old"
    docker image rm "meridian-tdoa:$old" >/dev/null 2>&1 || true
done
