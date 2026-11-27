#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p vendor
if [ ! -d vendor/kiwiclient ]; then
  git clone https://github.com/jks-prv/kiwiclient.git vendor/kiwiclient
fi
git -C vendor/kiwiclient checkout --detach 4eb733e6b6147f7fbeb97ced64cdac029b202d18
python3 -m venv .venv
.venv/bin/python -m pip install 'numpy==2.3.5'
.venv/bin/python vendor/kiwiclient/kiwirecorder.py --help > /dev/null
printf '%s\n' 'KiwiClient installed. Activate .venv before starting the server. Configure verified sources and access policies before enabling live capture.'
