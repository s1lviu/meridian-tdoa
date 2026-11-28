#!/bin/sh
# Deploy one commit to a host prepared once with deploy/host/setup.sh, over SSH.
#   scripts/deploy-host.sh SSH_HOST [GIT_REF]     (GIT_REF defaults to HEAD)
# Only committed files are shipped (git archive): the release is exactly that commit.
# The host builds and starts it with deploy/host/activate.sh and rolls back on a failed check.
set -eu
host=${1:?Usage: scripts/deploy-host.sh SSH_HOST [GIT_REF]}
ref=${2:-HEAD}
cd "$(dirname "$0")/.."
release=$(git rev-parse --short=12 --verify "$ref^{commit}")
echo "Deploying $release to $host"
git archive --format=tar "$release" |
    ssh "$host" "set -eu; d=/opt/meridian/releases/$release; rm -rf \"\$d\"; mkdir -p \"\$d\"; tar -x -C \"\$d\""
ssh "$host" "sh /opt/meridian/releases/$release/deploy/host/activate.sh $release"
