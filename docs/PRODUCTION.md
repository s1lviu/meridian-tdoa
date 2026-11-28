# Deployment and operation

[Back to Meridian](../README.md) · [Render](RENDER.md) · [Hosting options](HOSTING.md)

## Run

```sh
MERIDIAN_CONFIG=config/production.json cargo run --release --locked
```

All mutable state lives under `data/` (`data/meridian.sqlite`, `data/captures/`, `data/inbox/`). One process writes it; run a single instance. On first start, a former JSON archive (`data/results`, `data/jobs`) is imported once into SQLite.

With Docker, `scripts/deploy.sh your-domain.example` builds the image, generates an administrator token in `.secrets/runtime.env` and starts Compose behind Caddy (TLS). The container runs as UID 10001 with a read-only root filesystem; `/app/data` must be a persistent volume.

## The monitor

```json
"monitor": {
  "target_id": "dcf77-validation",
  "additional_target_ids": ["ft8-20m", "ft8-40m", "msf-channel", "ft8-30m", "dcf77-channel", "ft8-17m", "ft8-15m", "am-648khz", "am-5mhz"],
  "interval_s": 60,
  "directory_receivers": 12,
  "parallel_bands": 2
}
```

Every `interval_s` the monitor opens a window of `parallel_bands` targets on disjoint receivers; windows run in the background, so the next one starts on time while earlier ones are still processing (`max_parallel_jobs`, default 2). Windows with an FT8 target start 4 s before a 15 s UTC boundary so a full slot is recorded.

Directory-driven targets alternate reconnaissance with measurement. With `directory_receivers` set, every profile picks its receivers from the directory; `source_ids` then only matter as seeds for DCF77. Reconnaissance rotates a geographic anchor, except for DCF77: LF ground wave reaches a region, so its reconnaissance starts at the eligible receiver nearest to the mean position of its configured receivers. That point is derived from receiver positions only, never from the transmitter, and the solver never sees it. A receiver whose IQ lacks the PTB phase code in two captures within six hours is skipped on that channel. A generic-channel probe lasts up to 15 s. Measurement ranks sites by recent per-receiver RF strength on that channel, checks the first triangle for collinearity and favours the configured spacing. The scheduler progressively relaxes spacing if few sites are eligible. Two independent silent windows inside six hours remove a site from that channel temporarily. Reprocessing a recording counts as the original listen. The scheduler uses receiver coordinates and reception history.

Targets and their verification profiles:

| Profile | Kind | What establishes an emission |
|---|---|---|
| `dcf77_prn_v1` | `public_time_reference` | PTB phase code matched in each receiver's IQ; failing receivers are withheld, ≥3 must pass |
| `ft8_v1` | `amateur_digital` | FT8 decoded from our own IQ (CRC-14 + LDPC); identity from the message |
| `correlated_v1` | `monitored_channel` | CFAR-occupied sub-band correlated between synchronized receivers; identity unknown |
| `ncdxf_cw_v1` | `public_beacon` | Scheduled IBP callsign matched against all 18 Morse patterns |

## Fair use of public receivers

KiwiSDR operators publish whether external software may connect (`ext_api`) and offer TDoA channels (`tdoa_ch`); many also enforce a per-IP daily time limit. Meridian:

- checks `/status` (access policy, GNSS, antenna, free channels) immediately before every connection;
- keeps one connection per receiver at a time, at least `min_gap_s` (60 s) apart;
- budgets `daily_minutes` (60) of connection time per receiver per rolling 24 h. Connection time runs from the opened session to its end, so failed, timed-out and refused sessions count too;
- backs off exponentially after failures (10 min doubling, at most 24 h) and honours explicit refusals. Per-IP limits and password requirements set a 24 h block, a receiver reporting downtime sets at least 1 h and busy channels set at least 10 min;
- treats a KiwiSDR relay (`*.proxy.kiwisdr.com`, `proxy2`, `proxy3`) as one host. If it refuses the TCP connection to every receiver behind it in a capture, the relay is blocked with the same back-off and none of its receivers is contacted until the block ends.

```json
"receivers": {"daily_minutes": 60, "min_gap_s": 60}
```

`/api/receivers/health` shows the ledger. The earlier monitor, which had no such budget, exhausted several European receivers' daily limits within hours; that is why these rules exist.

## Storage and retention

```json
"retention": {
  "verified_iq_hours": 24,
  "unverified_iq_hours": 0,
  "capture_metadata_days": 30,
  "job_days": 30,
  "result_days": 90,
  "negative_result_days": 7
}
```

- Raw IQ of a capture with nothing worth keeping (no verified callsign, no transmission timed at two receivers, no correlated emission) is deleted as soon as processing ends. Verified IQ stays 24 h.
- Small evidence files (manifest, source checks, verifier scores, hashes) stay 30 days.
- Observations stay 90 days; coverage-only rows (nothing detected) 7 days; jobs 30 days.
- `MERIDIAN_CAPTURE_QUOTA_BYTES` (default 2 GiB) is a rolling quota: concurrent CLI and server captures reserve space before recording. The oldest eligible IQ is evicted first. A full quota skips a window and the scheduler tries again at its next interval.

The SigMF importer shares the capture quota. In-progress captures hold a filesystem reservation. A background sweep applies the retention rules every ten minutes. Back up `data/meridian.sqlite` with SQLite's online backup (`sqlite3 data/meridian.sqlite ".backup backup.sqlite"`) or stop the service and copy `data/`.

## Clock and time

`gps_utc_offset_s` (18 s) and `gps_offset_valid_until_utc` must follow the current IERS Bulletin C; capture refuses to start after the revalidation date. Kiwi IQ blocks carry GNSS timestamps checked in every block; SigMF nodes declare their time source and uncertainty.

## Acceptance on a new host

1. `cargo test --locked`, `python3 tests/detections.py target/release/meridian-tdoa`, `npm run test:browser`.
2. One manual capture per profile (`meridian-tdoa capture <target> --directory 12`), then `process`, and check `/api/receivers/health` for refusals.
3. `meridian-tdoa calibrate` (DCF77) and `meridian-tdoa calibrate ft8` once data exists: they score stored observations against the hold-out references.
4. Check retention: unverified captures keep only JSON after processing.

## Deployment scope

The native release has served stored over-the-air LF, MF and FT8 fixes from an isolated database copy. A Linux container deployment requires an image build on an available Docker daemon and persistent storage mounted at `/app/data`. The supplied Render Free Blueprint declares no disk. The volunteer monitor's long-run yield and latency need measurement on the chosen host and its receiver network. Use the per-band counts, coverage, receiver ledger and latency fields to evaluate that deployment.

Physical commissioning of a regional network requires simultaneous recordings or arrival reports from surveyed receiver sites and a separate hold-out transmitter. Public Kiwi coverage, a simulated GPSDO and regional MLAT tests exercise different portions of that system. [Field results](VALIDATION.md)

The RF pilot identifies local FT8 messages and DCF77, and can locate a sufficiently correlated anonymous sub-band. The broader architecture also proposed a WSPR decoder, automatic modulation classification, explicit delay–Doppler search and a propagation model fitted to HF paths. Those capabilities remain future work. Current HF field evidence covers the 40 m FT8 channel and a 5 MHz anonymous-channel trial. [Method and validation](VALIDATION.md)
