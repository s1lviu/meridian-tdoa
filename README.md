# Meridian TDoA Observatory

A Rust web application for locating eligible public beacon and participating amateur-radio emissions using **time difference of arrival only**, with an interactive map and an auditable IQ processing pipeline.

[![Verify Meridian](https://github.com/s1lviu/meridian-tdoa/actions/workflows/ci.yml/badge.svg)](https://github.com/s1lviu/meridian-tdoa/actions/workflows/ci.yml)
[![Deploy to Render](https://render.com/images/deploy-to-render-button.svg)](https://render.com/deploy?repo=https://github.com/s1lviu/meridian-tdoa)

**Status: experimental; live RF acceptance is still pending.** Real historical multireceiver IQ has been processed and tested in Chromium, including injected GNSS loss. Live source policies and metadata have been checked, but the attempted WebSocket IQ captures failed in the development environment. There is no claimed live reception, continuous global monitoring, or guaranteed localization accuracy. Publishing this repository does not deploy a running service.

![Meridian displaying the real DCF77 recording and its unresolved location ambiguity](evidence/desktop.png)

## What it does

- Filters estimated regions by viewport intersection when the map moves or zooms, with band and UTC filters.
- Keeps every confirmed receiving station in the selected emission's details, including receivers outside the viewport. Receivers actually used by the solver are distinguished visually.
- Requires at least three independent, non-collinear receivers observing the same RF emission simultaneously on a common timebase.
- Rejects inadequate synchronization, correlation or geometry with **“Position undetermined”**. Separates **“Unmonitored interval”** from **“No emission detected”**.
- Uses verifiable emission-specific identity evidence; otherwise shows **“unknown callsign”**. Frequency and estimated position never establish identity.
- Separates live capture, archived replay and injected-fault tests. Captures are requested by an administrator; map refreshes do not open SDR connections.
- Discovers the public KiwiSDR directory, checks the published API policy and receiver status, records independent IQ streams with the official KiwiClient, and runs DSP and localization in Rust.

## Real-data validation

The bundled dataset is the **DCF77 public time reference, recorded on 13 August 2020** by three receivers. It is a commissioning dataset, not an amateur-radio beacon recording. Original WAV files and SHA-256 hashes are preserved.

A global search produces **two compatible hypotheses**. Post-hoc distances from the independently published PTB reference are **1.7883 km** and **19,798.1022 km**. The application does not select the closer hypothesis: its result remains **Position undetermined**. The reference coordinates are used only by the evaluator, never by the solver, receiver selection or search domain.

Invalidating one receiver's GNSS state leaves two synchronized receivers and produces no position or candidate region. See the [validation report](docs/VALIDATION.md) and [live-source evidence](docs/LIVE-SOURCES.md).

## Quick start with Docker

```sh
git clone https://github.com/s1lviu/meridian-tdoa.git
cd meridian-tdoa
docker build -t meridian-tdoa:0.2.0 .
docker run --rm --init -p 3000:3000 \
  -e MERIDIAN_CONFIG=/app/config/app.json \
  -v meridian-data:/app/data meridian-tdoa:0.2.0
```

Open **http://localhost:3000/?replay=1**. This command uses the replay configuration. It needs no external map tiles, API keys or font services. The official KiwiClient archive and real recording are included in the source tree; dependency packages are downloaded during the build.

For live operation, use `config/production.json` and a protected administrator token. Follow [production setup](docs/PRODUCTION.md) before enabling acquisition. Historical availability is not evidence of current receiver access or synchronization.

## Deploy from GitHub

The Render button uses [`render.yaml`](render.yaml), which explicitly selects **Free** and creates no paid database or disk. Render handles the public URL and HTTPS. The generated `MERIDIAN_ADMIN_TOKEN` is available in the service's environment settings; never put it in a URL or commit it.

**Render Free loses new captures and job history on restart or idle shutdown.** It is suitable for testing the interface and attempting bounded live captures, not for a durable production archive. See [Render setup](docs/RENDER.md).

[Hosting alternatives](docs/HOSTING.md) compares Northflank Sandbox, Koyeb and other options using current provider documentation. No hosting provider has yet been validated for this application's live RF workload.

## Run natively

Requirements: the tested Rust toolchain is **1.100.0**, a C toolchain for dependencies, and Python 3 for evaluation. Node is required only for frontend dependency maintenance and browser tests.

```sh
cargo build --locked
./scripts/start.sh
```

`MERIDIAN_CONFIG` selects a configuration file; `PORT` overrides its listening port and binds `0.0.0.0`. The default `config/app.json` disables live capture. To install the native capture client:

```sh
./scripts/setup_kiwiclient.sh
MERIDIAN_CONFIG=config/live-verified.json \
  MERIDIAN_ADMIN_TOKEN_FILE=/path/to/protected/token ./scripts/start.sh
```

The token must contain at least 24 characters. The API does not expose it and the browser does not persist it. The Docker configuration uses `python3`; the native live configuration uses `.venv/bin/python`.

## Reproduce the checks

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

Set `BASE_URL` if the test server is not at `http://127.0.0.1:3000`. `tests/release.mjs` additionally exercises authenticated reprocessing against a running server and requires `MERIDIAN_ADMIN_TOKEN` or `.secrets/runtime.env`. Tests never contact public SDRs to obtain synthetic successes. The four-receiver geometry test is explicitly synthetic; it is not evidence of live accuracy.

## Configuration and data

Only verified **approximately 12 kHz Kiwi IQ WAV** is supported. Ordinary audio streams and unverified WebSDR/OpenWebRX protocols are not used for TDoA. Captures use 3–8 receivers, 10–120 seconds and at most 64 MiB of IQ per processed set, subject to stricter source limits. Published `ext_api` and `tdoa_ch` policies, band availability, antenna metadata and preflight GNSS are rechecked before acquisition. Synchronization is then checked in the actual IQ blocks.

The DCF77 profile verifies the PTB PRN modulation before automatic processing. Other targets require capture-specific eligibility evidence through the review API. The 1 km antenna-position allowance is a declared assumption, not a surveyed accuracy. Timing and uncertainty limitations are documented in [VALIDATION.md](docs/VALIDATION.md).

Import another real recording under `data/datasets/<name>/manifest.json`, using the bundled manifest as the schema example. Keep paths within the dataset directory, preserve original IQ hashes and use `mode: "replay"`. Set `emission_verified: true` only after checking provenance; `identity` may remain `null`.

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

Contributions: [CONTRIBUTING.md](CONTRIBUTING.md). License: **GPL-3.0-or-later** for this project; bundled upstream assets retain their own licenses and attribution in [THIRD_PARTY.md](THIRD_PARTY.md).
