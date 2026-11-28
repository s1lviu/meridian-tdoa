# Agent guide

Read this before changing Meridian. It is written for AI coding agents and applies equally to people. The linked docs hold the details.

## What this project is

Meridian locates radio transmitters by **time difference of arrival (TDoA)** in IQ recorded on synchronized SDRs, and shows them on a live map. Everything is pure RF: every position comes from our own timing measurements.

### Non-negotiable

These rules come from [CONTRIBUTING.md](CONTRIBUTING.md).

- **Positions come only from TDoA** on our own IQ or on arrival times reported by our nodes.
  - Known transmitter positions, published beacon locations and locators sent in FT8 messages are hold-out references for post-hoc evaluation only.
  - They are never solver inputs and never decide between hypotheses.
- **No spot feeds or self-reported locations.** That excludes PSK Reporter, Reverse Beacon Network, WSPRnet and MQTT.
- **Ambiguity stays visible.** An ambiguous solution stays ambiguous, and a unique fix needs at least four receivers.
- **Public KiwiSDRs are shared, volunteer-run receivers.**
  - Respect their published policy and their refusals (`too_busy`, `ip_limit`, `badp`), and stay within the per-receiver daily budget.
  - Tests and CI never contact public receivers.
- **Results are measured, never estimated.** Numbers in `README.md` and `docs/VALIDATION.md` must have evidence under `evidence/`. Do not estimate, round up or invent a result.

## Layout

| Path | Contents |
|---|---|
| `src/main.rs` | HTTP API, jobs, server-sent events, CLI subcommands |
| `src/monitor.rs` | Live scheduler: capture windows, targets, receiver choice |
| `src/capture.rs`, `src/kiwi_stream.rs`, `src/directory.rs` | Receiver preflight, KiwiSDR IQ capture, public receiver directory |
| `src/dsp.rs`, `src/toa.rs`, `src/ft8.rs`, `src/ft8_tdoa.rs`, `src/protocol.rs`, `src/channel.rs`, `src/ibp.rs` | Detection and arrival-time measurement for each signal profile |
| `src/geo.rs`, `src/pipeline.rs`, `src/tracks.rs`, `src/sync.rs` | Solver and uncertainty, emitter registry, node clock synchronization |
| `src/db.rs`, `src/retention.rs` | SQLite store, fair-use ledger, IQ retention |
| `src/bin/meridian-node.rs` | Edge node agent (SigMF, optional SoapySDR) |
| `web/` | The map: plain JavaScript and Leaflet, no build step. `web/vendor/` is copied from `node_modules` by `npm run vendor` |
| `tests/` | Rust integration tests, Python and Playwright end-to-end checks, container acceptance |
| `config/` | `app.json` (default, no capture) and `production.json` (live monitor) |
| `deploy/` | Own server: `host/` (compose with resource limits, setup, release activation) and a Caddy example for a dedicated server |
| `docs/`, `evidence/` | Documentation and measured results |
| `.github/workflows/ci.yml` | CI: the `verify` job and the `container` job on x86_64 |

The full table is in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#layout).

## Setup

- **Toolchain:**
  - Rust 1.100.0 with rustfmt, pinned in `rust-toolchain.toml`;
  - a C compiler, because the build compiles ft8_lib and SQLite;
  - Python 3;
  - Node 22.
- **Browser checks:** run `npm ci`, then `npx playwright install chromium`, or set `CHROMIUM_PATH` to a Chromium binary.
- **Local `.env`:** a git-ignored `.env` can hold toolchain paths and `CARGO_TARGET_DIR` (see [.env.example](.env.example)). Source it before you run cargo: `. ./.env`.
- **Binary paths:** with `CARGO_TARGET_DIR` set, binaries are not under `target/`. Use `${CARGO_TARGET_DIR:-target}/debug/meridian-tdoa`.

## Verify a change

```sh
scripts/check.sh           # what the CI verify job runs
scripts/check.sh --quick   # rustfmt and cargo test only
```

`scripts/check.sh` runs, in order:
1. rustfmt;
2. `cargo test`;
3. the KiwiClient transport check;
4. node detections against a real server process;
5. the SoapySDR node, when libSoapySDR is installed;
6. the Playwright checks of the map.

It reads `.env` for toolchain paths only.

- **Docker image:** `tests/container.sh` checks it (needs Docker). CI runs it on x86_64.
- **Screenshots:** `tests/browser.mjs` writes screenshots and `browser-tests.json` to `evidence/` unless `MERIDIAN_EVIDENCE_DIR` names another directory, and `scripts/check.sh` uses a temporary one. Regenerate the committed screenshots only for an intended UI change.
- **Test servers:** tests start the server from a temporary directory, so resolve binary paths to absolute ones first.
- **UI changes:** check desktop and 390 px mobile, and extend `tests/browser.mjs`.
- **Regression tests:** changes to capture, GNSS parsing, DSP or localization need one.

Individual commands are in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#checks).

## Run it

```sh
cargo run --locked                                                   # config/app.json: no capture from public receivers
MERIDIAN_CONFIG=config/production.json cargo run --release --locked  # live monitor: contacts public KiwiSDRs
```

Start the live profile only when you mean to capture, because it uses real receivers' time. `MERIDIAN_DIRECTORY_OFFLINE=1` stops directory refreshes.

The map shows live data only. The archived 2020 DCF77 recording under `data/recordings/` is a test fixture: the Rust tests read it, and the container copies it into `data/` only with `MERIDIAN_SEED_RECORDING=1` (`tests/container.sh`).

## Deployment and git

- **`main` deploys itself.** It goes to Render (Free, Frankfurt) once all GitHub checks pass (`render.yaml`, `autoDeployTrigger: checksPass`).
- **Every deploy wipes data.** It restarts the service, and Free has no disk, so observations collected so far are lost. Batch changes and merge on purpose.
- **Own server.** `scripts/deploy-host.sh SSH_HOST [REF]` deploys one commit to a host prepared once with `deploy/host/setup.sh` ([docs/HOSTING.md](docs/HOSTING.md#own-server-behind-apache)). It builds on the host, waits for the health check and rolls back on failure.
- **Branches only.** Work on a branch and open a pull request; never push to `main`.
- **Secrets.** Never commit `.env`, tokens, captures or `.secrets/`. The administrator token comes from `MERIDIAN_ADMIN_TOKEN_FILE` or `MERIDIAN_ADMIN_TOKEN`.

## Conventions

- **Language:** English for code, interface text, docs, commits and pull requests.
- **Commits:** an imperative summary line in sentence case, with no type prefix. The body says what changed and how it was verified.
- **Formatting:** Rust is formatted by `cargo fmt`. JavaScript, CSS, HTML, JSON and shell use 2-space indentation (`.editorconfig`).
- **API changes:** update [docs/API.md](docs/API.md). Measured results go in [docs/VALIDATION.md](docs/VALIDATION.md) together with their evidence file.
