#!/bin/sh
# Run locally what the CI verify job runs (.github/workflows/ci.yml), in the same order.
#   scripts/check.sh           rustfmt, Rust tests, transport, node detections, SoapySDR, browser
#   scripts/check.sh --quick   rustfmt and Rust tests only
# A local .env (git-ignored) is read for toolchain paths. The browser check writes its
# screenshots to a temporary directory unless MERIDIAN_EVIDENCE_DIR is set.
# No public receiver is contacted.
set -eu
cd "$(dirname "$0")/.."
if [ -f .env ]; then
  . ./.env
fi
# Runtime settings from .env must not leak into the test servers.
unset MERIDIAN_CONFIG MERIDIAN_ADMIN_TOKEN MERIDIAN_ADMIN_TOKEN_FILE MERIDIAN_CAPTURE_QUOTA_BYTES MERIDIAN_KIWI_TRANSPORT PORT
quick=0
if [ "${1:-}" = "--quick" ]; then
  quick=1
fi
target="${CARGO_TARGET_DIR:-target}"
step() { printf '\n== %s\n' "$*"; }

step "rustfmt"
cargo fmt -- --check
step "Rust tests"
cargo test --locked
if [ "$quick" = 1 ]; then
  printf '\nQuick checks passed.\n'
  exit 0
fi

step "KiwiClient transport"
python3 tests/transport.py
step "Node detections through a real server"
cargo build --locked
python3 tests/detections.py "$target/debug/meridian-tdoa"
if command -v pkg-config > /dev/null && pkg-config --exists SoapySDR; then
  step "SoapySDR node"
  cargo build --locked --features soapy --bin meridian-node
  python3 tests/soapy.py "$target/debug/meridian-node"
else
  printf '\n== SoapySDR node: skipped, libSoapySDR not found (CI runs it)\n'
fi

step "Map in Chromium"
[ -d node_modules/playwright ] || npm ci
if [ -z "${CHROMIUM_PATH:-}" ]; then
  bundled="$(node --input-type=module -e 'import { chromium } from "playwright"; process.stdout.write(chromium.executablePath())')"
  if [ ! -x "$bundled" ]; then
    echo "Chromium for Playwright is missing: run 'npx playwright install chromium' or set CHROMIUM_PATH." >&2
    exit 1
  fi
fi
evidence="${MERIDIAN_EVIDENCE_DIR:-$(mktemp -d)}"
MERIDIAN_EVIDENCE_DIR="$evidence" node tests/browser.mjs "$target/debug/meridian-tdoa"
printf 'Screenshots: %s\n' "$evidence"
printf '\nAll checks passed.\n'
