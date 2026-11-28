# Meridian API

JSON over the same origin as the map. Times are RFC 3339 UTC. Request bodies are limited to 16 KiB (256 KiB for `/api/detections`). No permissive CORS.

| Method | Route | Purpose |
|---|---|---|
| GET | `/api/health`, `/api/ready` | Process health; readiness with version, `busy` and free workers |
| GET | `/api/info` | Public configuration, targets, bundled recordings, newest live observation |
| GET | `/api/monitor` | Scheduler state only: `next_at`, `next_target_ids`, `interval_s`, pause `notice`, `last_jobs`; `null` without a monitor |
| GET | `/api/tracks` | Emitter registry: fused positions with 95% ellipses (`since`, `band`, `bbox`) |
| GET | `/api/tracks/{key}` | One emitter with its contributing observations |
| GET | `/api/emissions` | Observations by time, band, mode and viewport |
| GET | `/api/emissions/{id}` | One observation with every receiver, pair and evidence |
| GET | `/api/emissions/{id}/evaluation` | Post-hoc distance to the hold-out reference, where one exists |
| GET | `/api/stats` | Per profile and band, counts and fixes/h, P50/P90 errors, 68%/95% coverage, publication latency (`start`, `end`) |
| GET | `/api/stream` | Server-sent `change` events when observations or tracks change |
| GET | `/api/receivers/health` | Fair-use ledger: per-receiver successes, failures, block state, minutes in 24 h |
| GET | `/api/clocks` | Node clock filters (reference-emitter synchronization) |
| GET | `/api/sources`, `/api/beacons` | Public receiver directory and exclusion reasons |
| POST | `/api/detections` | Arrival-time reports from own nodes (node bearer token) |
| POST | `/api/capture` | Capture a configured target now (administrator token) |
| POST | `/api/replay` | Open or reprocess a bundled recording |
| POST | `/api/review` | Attach eligibility evidence to a manual capture (administrator token) |
| POST | `/api/directory/refresh` | Refresh the receiver directory (administrator token) |
| GET | `/api/jobs/{id}` | Job status |

## Observations

```text
GET /api/emissions?mode=live&compact=true&start=2026-11-27T18:00:00Z&end=2026-11-27T19:00:00Z&band=40m&bbox=-10,35,30,60
```

- `band`: `all` (0–6 GHz), `lf`, `mf`, `hf`, `vhf`, `uhf`, `shf` or an amateur band name (`40m`, `2m`, …).
- `mode`: `live` or `replay`; `scenario`: `normal` or `sync-loss` (fault-injection replays are kept apart).
- `bbox=west,south,east,north` filters by region polygons; `west > east` crosses the antimeridian.
- `emissions` intersect the viewport; `unlocated` are observations without any region (heard, not located); `monitoring` lists the captured intervals and gaps.

`status`: `located` (one solution, `estimate`), `undetermined` (`estimate=null`, possibly several `hypotheses`), `no_detection`, `no_verified_beacon`. `reference.profile` identifies how the emission was established: `ft8_v1`, `dcf77_prn_v1`, `correlated_v1`, `mlat_v1`, `ncdxf_cw_v1`.

Each estimate carries `center`, the 95% `polygon` and `radius_km`, `residual_us`, the a-posteriori `scale`, `dof` and a local east/north `covariance_km2` `[σee², σen, σnn²]`. Pairs are `delay_us` (arrival B minus A) with `sigma_us`; FT8 pairs come from per-receiver arrival times, so they close cycles exactly.

## Emitter registry

`GET /api/tracks?since=2026-11-27T00:00:00Z&band=40m` returns `tracks[]` with `track` (`key`, `callsign`, `profile`, `band`, `frequency_hz`, `center`, `covariance_km2`, `fixes`, `rejected`, `first_utc`, `last_utc`, `rf_utc`, `observations`) and `ellipse_95` (`major_km`, `minor_km`, `azimuth_deg` of the major axis from north). Keys are `call:<CALLSIGN>` for decoded identities, `ref:DCF77`, or `obs:<observation_id>` for an anonymous emission. Sharing a frequency does not establish transmitter identity. Fixes are fused by covariance intersection; a fix outside the 99.9% gate is counted in `rejected` and not fused.

## Statistics

`GET /api/stats?start=…&end=…` includes every live observation whose RF start falls in the half-open UTC interval. The rate divides the number of unique fixes by the full interval in hours. Error and coverage fields include their evaluation counts and use independent references where available. P50 averages the two middle values for an even sample. P90 uses nearest rank. The 68% coverage uses the local two-dimensional covariance ellipse and the 95% coverage uses the reported contour.

`publication_latency_count`, `median_publication_latency_s` and `p90_publication_latency_s` measure from RF time to the first stored fix. Importing archived observations does not establish an original publication time. A node network without a UTC master can report relative delays but supplies no absolute RF latency. The map also shows RF-to-display time for new observations received on an open foreground tab, measured after the rendering frame. This session metric depends on the client clock.

## Push updates

`GET /api/stream` is an `EventSource` stream. Each `change` event carries `{"latest_live", "live_rows", "tracks_updated"}`; the map refreshes on it instead of polling. Streams end when the server shuts down.

The map's countdown reads `GET /api/monitor`, the same scheduler snapshot as `observatory` in `/api/emissions`. When `next_at` has passed, the scheduler is starting that window and has not yet announced the following one, so the map asks again every 2 s, at most 30 times. With `next_at: null`, a `notice` explains why the monitor paused.

## Node detections

```http
POST /api/detections
Authorization: Bearer <node token>

{"frequency_hz": 145800000, "detections": [{"node": "n0", "signal": "beacon-42", "local_s": 1795852525.0012345, "sigma_s": 1e-7}]}
```

The token must match a configured node's `token_sha256` and every detection must name that node. Reports with the same nominal frequency and signal identifier are processed after 2 s of inactivity. A late report within 120 s can update the same observation. The local FT8 detector uses the decoded payload and slot in the identifier. A configured reference updates the clock filters. Other reports can produce `mlat_v1` observations. See [nodes](NODES.md).

## Capture and jobs

```http
POST /api/capture
Authorization: Bearer <administrator token>

{"target_id": "ft8-20m", "source_ids": ["ID1", "ID2", "ID3", "ID4"]}
```

`source_ids` (3–12 directory ids) is optional. Several jobs can run at once (`max_parallel_jobs`) on disjoint receivers; a full house returns 409. Jobs are durable: `running`, `completed`, `awaiting_review`, `failed`, `interrupted` (after a restart; never retried automatically). Slot profiles (`ft8_v1`, `ncdxf_cw_v1`) and channels (`correlated_v1`) report `sampled_slots`/`decodes`/`messages`/`multi_receiver`/`located` or `segments`/`detected`/`located`.

The administrator token (≥24 characters) comes from `MERIDIAN_ADMIN_TOKEN_FILE` or `MERIDIAN_ADMIN_TOKEN`; it is never returned or stored by the browser.
