# Meridian TDoA

**Live transmitter locations measured from synchronized radio recordings.**

Meridian records the same transmission on several synchronized software-defined radios, measures the differences in arrival time (TDoA) and plots where the transmitter can be. Every position comes from our own IQ samples: no spot feeds, no self-reported locations, no MQTT.

[![Verify Meridian](https://github.com/s1lviu/meridian-tdoa/actions/workflows/ci.yml/badge.svg)](https://github.com/s1lviu/meridian-tdoa/actions/workflows/ci.yml)

[What it locates](#what-it-locates-and-how-well) · [Quick start](#quick-start) · [Any SDR](#any-sdr) · [Documentation](#documentation)

## What it locates, and how well

Field results use recordings from public receivers. The node row describes a controlled hardware-interface test. See the [validation report](docs/VALIDATION.md).

| Signal | How it is measured | Measured so far |
|---|---|---|
| **DCF77, 77.5 kHz** (LF ground wave) | Correlation between receivers, phase code verified per receiver | 32 four-receiver fixes, median **10.5 km**, 95% region contains the reference in **32/32** |
| **FT8 amateur stations** (HF, decoded from our own IQ) | Each receiver timed against the waveform rebuilt from the decoded message | Two fixes, **194 km** and **274 km** from the centres of the transmitted locator squares. 595 decoded observations across 16 captures. |
| **Unknown emitters** in a monitored channel | CFAR finds occupied sub-bands; each is correlated and located | On 648 kHz two independent four-receiver windows yielded 95% regions with radii of **438 km** and **970 km**. Live LF and 5 MHz trials are recorded in the validation report. |
| **Own nodes** (any SDR) | Arrival times reported by edge nodes, clocks disciplined by GNSS or synchronized from reference emitters | Verified end to end with simulated nodes |

The physics sets the scale: LF ground wave reaches kilometres; HF paths via the ionosphere and narrowband signals give hundreds of kilometres per fix. Meridian publishes a unique position only with **four or more receivers** and one bounded solution. Receivers on one side of a station can leave the range poorly constrained.

## How it works

1. **Capture** — public KiwiSDRs (GNSS-timestamped IQ, used within their operators' published policy and a per-receiver daily budget) or your own SDR nodes (SigMF recordings, or arrival-time reports).
2. **Detect** — decode FT8 locally, verify DCF77's phase code, or find occupied sub-bands with CFAR.
3. **Time** — matched filter against the rebuilt waveform, or Doppler-tolerant cross-correlation, with an uncertainty estimated from the data itself.
4. **Locate** — all receiver pairs reduced to independent differences (generalized least squares on the WGS84 ellipsoid), global search, 95% region scaled by the measurements' own consistency.
5. **Track** — successive fixes of the same transmitter fused by covariance intersection into one emitter on the map.

The map receives changes as they happen (server-sent events). Raw IQ is kept only while useful (24 h when verified, deleted immediately otherwise); results and the emitter registry live in SQLite.

## Quick start

Build with Rust 1.100 and a C compiler. Direct KiwiSDR capture uses the native Rust client. The optional KiwiClient transport requires Python 3 with NumPy.

```sh
MERIDIAN_CONFIG=config/production.json cargo run --release --locked
```

Open [localhost:3000](http://localhost:3000/). The production profile monitors DCF77, MSF, two public AM channels (648 kHz and 5 MHz) and five FT8 bands, choosing receivers from the public directory. The bundled 2020 DCF77 recording opens with [`/?replay=1`](http://localhost:3000/?replay=1) and needs no network.

With Docker:

```sh
docker build -t meridian-tdoa:0.3.0 .
docker run --rm --init -p 3000:3000 -e MERIDIAN_CONFIG=/app/config/app.json -v meridian-data:/app/data meridian-tdoa:0.3.0
```

Single measurements from the command line:

```sh
meridian-tdoa capture ft8-40m --directory 12 --near 48,10   # pick 12 spread receivers near a point
meridian-tdoa process data/captures/<id>/manifest.json
meridian-tdoa calibrate ft8                                 # post-hoc accuracy against sent locators
```

## Any SDR

TDoA is a clock problem first. An SDR can contribute when its samples carry time:

- **GNSS-disciplined** (USRP with GPSDO, any SDR with a PPS-stamped recorder, KiwiSDR): use `meridian-node capture` with a compatible SoapySDR GPSDO driver, or record SigMF with `core:datetime` anchors and drop the files in `data/inbox/<event>/` (or `meridian-tdoa ingest DIR`).
- **Free-running** (RTL-SDR, Airspy, HackRF, SDRplay): report arrival times with `meridian-node`; the server synchronizes the clocks from emitters with published positions, from matching reports of the same reference signal.
- **Not usable for TDoA:** audio-only web receivers (WebSDR, OpenWebRX) — they carry no sample timing.

See [any-SDR nodes](docs/NODES.md).

## Documentation

| I want to… | Read |
|---|---|
| Understand the measurements and their limits | [Method and validation](docs/VALIDATION.md) |
| Run it, configure receivers, retention and the monitor | [Deployment and operation](docs/PRODUCTION.md) |
| Add my own SDR nodes | [Nodes, SigMF and detections](docs/NODES.md) |
| Integrate with the backend | [API reference](docs/API.md) |
| Use public KiwiSDRs fairly | [Live sources](docs/LIVE-SOURCES.md) |
| Build and test | [Development guide](docs/DEVELOPMENT.md) |
| Work on the code with an AI coding agent | [Agent guide](AGENTS.md) |

## License

[GPL-3.0-or-later](LICENSE). Bundled recordings, libraries and data keep their own licenses: [third-party notices](THIRD_PARTY.md).
