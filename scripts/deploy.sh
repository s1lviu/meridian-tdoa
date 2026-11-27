#!/bin/sh
# Run on the destination host after extracting the release into an owned directory.
set -eu
cd "$(dirname "$0")/.."
domain=${1:-}
if [ -z "$domain" ] || ! printf '%s' "$domain" | LC_ALL=C grep -Eq '^[a-zA-Z0-9][a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$'; then
    printf '%s\n' 'Usage: ./scripts/deploy.sh domain.example (DNS must point to the server)' >&2
    exit 1
fi
docker compose version >/dev/null
# This public source configuration is bind-mounted into the unprivileged container.
# Keep credentials in .secrets/runtime.env, never in this file.
chmod a+r config/production.json
umask 077
mkdir -p .secrets
if [ ! -f .secrets/runtime.env ]; then
    python3 -c 'import secrets; print("MERIDIAN_ADMIN_TOKEN=" + secrets.token_urlsafe(32))' > .secrets/runtime.env
fi
printf 'MERIDIAN_DOMAIN=%s\n' "$domain" > .env
docker compose config --quiet
docker compose up -d --build --wait --wait-timeout 300
curl --fail --show-error --silent --max-time 20 "https://$domain/api/ready"
printf '\n%s\n' "The application responds at https://$domain. The administrator token is in .secrets/runtime.env on the server."
