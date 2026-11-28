# Development guide

[Back to Meridian](../README.md)

Requirements: Rust 1.100.0, a C compiler (ft8_lib and SQLite are compiled by `build.rs`/`rusqlite`), Python 3 (optional KiwiClient transport and integration tests), Node 22 with Playwright for browser tests.

```sh
./scripts/start.sh                       # build and serve with config/app.json (no capture)
MERIDIAN_CONFIG=config/production.json cargo run --release --locked
```

`MERIDIAN_CONFIG` selects the configuration; `PORT` overrides the listen address; `MERIDIAN_DIRECTORY_OFFLINE=1` disables directory refreshes.

## Commands

```sh
meridian-tdoa                                    # server
meridian-tdoa capture TARGET [--sources ID,… | --directory N [--near LAT,LON]]
meridian-tdoa process data/captures/<id>/manifest.json [FAULT_RECEIVER]
meridian-tdoa ingest DIR [--profile correlated_v1|ft8_v1] [--center HZ]
meridian-tdoa calibrate                          # stored DCF77 vs the PTB position (post hoc)
meridian-tdoa calibrate ft8                      # stored FT8 fixes vs the locators stations sent
meridian-tdoa import [OBSERVATIONS.json]           # import local history without serving
meridian-tdoa stats [START_UTC END_UTC]            # complete interval statistics
meridian-node capture SETTINGS.json OUT           # optional SoapySDR hardware input
meridian-tdoa discover                           # refresh the receiver directory
meridian-node watch DIR --server URL --node ID --token-file FILE [--once]
scripts/deploy-host.sh SSH_HOST [GIT_REF]          # deploy a commit to a prepared host (HOSTING.md)
```

Only the invocation without a subcommand starts the server.

## Checks

`scripts/check.sh` runs the steps of the CI `verify` job in order (`--quick`: rustfmt and the Rust tests). It reads a local `.env` for toolchain paths, skips the SoapySDR step when the library is missing and writes browser screenshots to a temporary directory. The individual commands:

```sh
cargo fmt -- --check
cargo test --locked                                   # unit, synthetic and real-IQ tests
# Optional GPSDO driver path needs pkg-config, a C++17 compiler and libSoapySDR.
cargo build --locked --features soapy --bin meridian-node
python3 tests/soapy.py target/debug/meridian-node     # software device through the actual library
python3 tests/transport.py                            # KiwiClient wrapper: one attempt, no redirects
python3 tests/detections.py target/debug/meridian-tdoa   # node detections through a real server
CHROMIUM_PATH=… npm run test:browser                  # map against a real server; screenshots to evidence/ or $MERIDIAN_EVIDENCE_DIR
docker build -t meridian-ci . && sh tests/container.sh meridian-ci  # offline Linux release and persistent volume
```

`tests/container.sh` runs the image non-root with a read-only root, 1 GiB memory and 128 pids, reprocesses the bundled DCF77 IQ and replaces the container on the same volume. It passed on linux/arm64 and linux/amd64 images on 27–28 November 2026 (UTC), and the Rust suite passed inside linux/amd64. To build linux/amd64 on Apple Silicon, use Rosetta binfmt in the Linux VM and remove `qemu-user`. BuildKit otherwise runs the Rust toolchain under QEMU, where `rustc -vV` crashes with SIGSEGV.

What the Rust tests establish (no public receiver is contacted):

| Area | Test |
|---|---|
| Kiwi parsing and GNSS | truncation, stale solutions, time jumps on the real 2020 recording |
| DCF77 | real IQ matches the PTB code; carrier and noise do not; a failing receiver is withheld, the capture proceeds |
| Resampling | Kaiser sinc keeps a 4.5 kHz tone within 10⁻³ where linear interpolation errs by >0.1 |
| Occupancy | CFAR finds a narrow carrier and a 300 Hz signal; noise alone gives nothing |
| Correlation | sub-windows survive a 0.5 Hz Doppler offset that defeats coherent 2 s windows |
| Solver | Monte Carlo 95% coverage ≥90% for per-receiver and pair noise (measured 60/60); three receivers keep both mirror solutions |
| FT8 | upstream WSJT-X reference recordings decode (4/5, 16/24); synthesized waveforms round-trip; decoder lag measured |
| Matched filter | rms timing error / jackknife σ = 1.04 over 40 noisy trials |
| FT8 TDoA | five-receiver synthetic Kiwi network: 0.64 km error |
| SigMF | anchors with 2 ppm clock error, channelization, nanosecond time alignment; unknown emitter located 0.08 km from truth; ingest builds a hashed manifest |
| Edge nodes | five SigMF nodes report FT8 arrivals; server-side association locates within 0.81 km |
| Clock sync | free-running clocks synchronized from a reference emitter; target within 29 m |
| Storage | SQLite queries, bundle replacement, pruning, interrupted jobs; retention deletes unverified IQ, evicts the oldest under quota |
| Tracks | covariance intersection, gating, duplicate observations |

Synthetic tests are geometry and algorithm checks, not evidence of live accuracy; see [VALIDATION.md](VALIDATION.md) for measured results.

## Layout

| Path | Responsibility |
|---|---|
| `src/kiwi.rs`, `src/sigmf.rs`, `src/iq.rs` | Timestamped IQ from KiwiSDR WAV or SigMF |
| `src/dsp.rs`, `src/toa.rs` | Resampling, occupancy (CFAR), correlation, matched-filter arrival time |
| `src/ft8.rs`, `src/ft8_tdoa.rs`, `third_party/ft8_lib/` | FT8 decoding and synthesis; FT8 TDoA profile; edge processing |
| `src/protocol.rs`, `src/ibp.rs`, `src/channel.rs` | DCF77, IBP and unknown-emitter profiles |
| `src/geo.rs`, `src/pipeline.rs` | WGS84 GLS solver, uncertainty, receiver subsets |
| `src/tracks.rs`, `src/sync.rs` | Emitter registry; reference-emitter clock synchronization |
| `src/db.rs`, `src/retention.rs` | SQLite store, fair-use ledger, retention |
| `src/capture.rs`, `src/directory.rs`, `src/main.rs`, `src/monitor.rs` | Acquisition, directory, API, scheduler |
| `src/bin/meridian-node.rs` | Edge node agent |
| `web/` | Map |

## Contributing

Keep the measurement contract in [CONTRIBUTING.md](../CONTRIBUTING.md): positions come only from our own timing measurements; known transmitter positions and sent locators belong only in post-hoc evaluation and calibration.
