# Development guide

[Back to Meridian](../README.md)

## Run natively

Requirements: the tested Rust toolchain is **1.100.0**, a C toolchain for dependencies, and Python 3 for evaluation. Node is required for frontend dependency maintenance and browser tests. Run commands from the repository root.

```sh
./scripts/start.sh
```

The script builds the application and starts the server. Open [localhost:3000](http://localhost:3000/?replay=1) for the included recording.

`MERIDIAN_CONFIG` selects a configuration file; `PORT` overrides its listening port and binds `0.0.0.0`. The default `config/app.json` disables live capture. To install the native capture client:

```sh
./scripts/setup_kiwiclient.sh
MERIDIAN_CONFIG=config/live-verified.json \
  MERIDIAN_ADMIN_TOKEN_FILE=/path/to/protected/token ./scripts/start.sh
```

The token must contain at least 24 characters. The API does not expose it and the browser does not persist it. The Docker configuration uses `python3`; the native live configuration uses `.venv/bin/python`. See [deployment and operation](PRODUCTION.md) for receiver configuration and capture requirements.

For the public live view, enable the [scheduled observatory](PRODUCTION.md#scheduled-public-observatory) in the native config and open the site root. The service acquires independently of browser sessions.

## Run the checks

```sh
cargo fmt -- --check
cargo test --locked
./scripts/validate_data.sh
python3 tests/transport.py
npm ci
npx playwright install --with-deps chromium
# Start the server in another terminal, then:
CHROMIUM_PATH="$(node --input-type=module -e 'import {chromium} from "playwright"; process.stdout.write(chromium.executablePath())')" npm run test:browser
```

Set `BASE_URL` if the test server is not at `http://127.0.0.1:3000`. `tests/release.mjs` additionally exercises authenticated reprocessing against a running server and requires `MERIDIAN_ADMIN_TOKEN` or `.secrets/runtime.env`.

Run `npm run test:browser:live` against a running monitor to check pinning, receiver comparisons, live evaluation and desktop/mobile layout. With the multiband profile it also tests all five scheduled beacon channels, the 18-beacon roster, directory overlays, band isolation and a real worker change. When `digital_reports` is enabled, it checks both live modes, clicks a digital field through the receiver layer, and checks band and UTC filtering. Set `CHROMIUM_PATH` to the browser executable. The check requires two verified LF windows in the last hour including one with four contributing receivers. It issues only read requests.

The [CI workflow](../.github/workflows/ci.yml) uses the bundled real recording and a fixed directory snapshot. Tests do not acquire new RF from public SDRs. The four-receiver geometry test is synthetic and is not evidence of live accuracy. See [the validation report](VALIDATION.md) for the measured results and interpretation.

## Recording format and capture configuration

Only verified **approximately 12 kHz Kiwi IQ WAV** is supported. Ordinary audio streams and unverified WebSDR/OpenWebRX protocols are not used for TDoA. Captures use 3–8 receivers, 10–120 seconds and at most 64 MiB of IQ per processed set, subject to stricter source limits.

Published `ext_api` and `tdoa_ch` policies, band availability, antenna metadata and preflight GNSS are rechecked before acquisition. Synchronization is then checked in the actual IQ blocks.

DCF77 verifies the PTB phase modulation. The IBP profile verifies each complete callsign in its ten-second slot and persists a result bundle for each capture. `cargo test --locked` exercises the schedule rollover, different Morse calls, pure-carrier rejection, differential arrival time, directory rotation and per-band evaluation. Other targets use capture-specific evidence through the [review API](API.md). Timing and uncertainty are documented in [VALIDATION.md](VALIDATION.md).

## Import a recording

Place another real recording under `data/datasets/<name>/manifest.json`, using the [bundled manifest](../data/recordings/manifest.json) as the schema example.

- Keep paths within the dataset directory and preserve original IQ hashes.
- Use `mode: "replay"` to distinguish an archived recording from a live capture.
- Set `emission_verified: true` only after checking provenance; `identity` may remain `null`.

```sh
cargo run --locked -- process data/datasets/name/manifest.json
# Restart the server to discover an imported dataset and result.
```

## Project layout

| Path | Responsibility |
|---|---|
| `src/kiwi.rs`, `src/dsp.rs` | Kiwi WAV parsing, GNSS validation, detection and correlation |
| `src/geo.rs`, `src/pipeline.rs` | TDoA search, geometry, ambiguity and uncertainty regions |
| `src/capture.rs`, `src/protocol.rs` | Independent capture and RF eligibility verification |
| `src/directory.rs`, `src/store.rs` | Public source metadata and durable single-writer state |
| `src/main.rs`, `web/` | Axum API and responsive Leaflet interface |
| `data/recordings/`, `evidence/` | Original real IQ, provenance and timestamped test evidence |
| `docs/API.md`, `docs/PRODUCTION.md` | API contract and operational setup |

## Contributing

Follow [CONTRIBUTING.md](../CONTRIBUTING.md). Keep original recordings and their provenance intact, and keep known transmitter coordinates confined to post-hoc evaluation.
