# Meridian API

JSON over the same origin as the UI. Times use RFC 3339 with an explicit timezone; the UI sends UTC `Z`. Request bodies are limited to 16 KiB. No permissive CORS is enabled.

| Method | Route | Purpose |
|---|---|---|
| GET | `/api/health` | Process health |
| GET | `/api/ready` | Readiness; 503 while stopping, otherwise version and worker state |
| GET | `/api/info` | Public configuration, datasets and pending captures; no token |
| GET | `/api/sources` | Paginated public directory and exclusion reasons |
| GET | `/api/beacons` | Official channel and callsign order, server UTC and mapped receiver directory entries with HF eligibility |
| GET | `/api/activity` | Shared live FT8/WSPR report counts by mode, band and two-letter transmitter field |
| GET | `/api/emissions` | Geographic, temporal, band and origin filtering |
| GET | `/api/emissions/{id}` | Full result, all receiver observations and evidence |
| GET | `/api/emissions/{id}/evaluation` | Post-hoc distances to the published reference for each solution of a verified live observation |
| POST | `/api/replay` | Open a cached result or reprocess an allowed local dataset |
| POST | `/api/capture` | Capture a configured eligible target; administrator bearer token |
| POST | `/api/review` | Attach emission-specific eligibility evidence; administrator token |
| POST | `/api/directory/refresh` | Refresh directory, respecting the 1800 s cache; administrator token |
| GET | `/api/jobs/{id}` | Job status |

## Filtering

```text
GET /api/emissions?bbox=-2,45,15,55&band=lf&start=2020-08-13T06:52:20Z&end=2020-08-13T06:52:55Z&mode=replay&scenario=normal
```

- `bbox=west,south,east,north`: WGS84 degrees; west > east crosses the antimeridian. Intersection checks polygons, not just centres.
- `band` accepts `all`, `lf`, `mf`, `hf` and the amateur bands in `/api/info.band_ranges_mhz`. The digital map also groups unclassified reports under `other`.
- `start`, `end`: half-open interval, start < end.
- `mode`: `replay` (default) or `live`.
- `scenario`: `normal` or `sync-loss`; injected faults are separate from normal results.
- `compact=true` returns map contours, capture ID/time, identity, signal profile, confirmed count and contributing receiver IDs. The detail route returns the complete measurement.

Response fields:

- `emissions`: estimated regions/hypotheses intersecting the viewport. A contour does not imply a unique position; inspect `status`.
- `unlocated`: undetermined observations without contours. They cannot be filtered geographically and are not plotted at a receiver or map centre.
- `total_matching`: detected emissions matching the non-geographic filters.
- `monitoring.segments`: captured intervals and channels, independent of the viewport.
- `monitoring.gaps`: the temporal complement of recorded segments in the selected interval.
- `history` contains up to 120 verified reference observations in compact responses. `signals` lists confirmed receptions for the selected band and interval, grouped by profile, documented callsign and channel. Each unnamed, reviewed emission outside a reference profile keeps its own observation ID. `confirmed_receivers`, `accepted_pair_count` and `pair_count` describe the RF evidence. For a fixed reference beacon, `last_position` can retain an earlier measured contour on the same channel if the latest reception had none. Both arrays are independent of the viewport.
- `band_stats` compares channels worldwide in the selected UTC interval for compact live requests. Each row includes scheduled attempts, analysed and verified slots, distinct beacons, unique fixes, ambiguous windows, number evaluated and median/P90 error in km. Empty error samples have JSON `null` percentiles. P90 uses the nearest rank.
- `observatory` is null when scheduling is off. Otherwise `last_jobs` lists the jobs in the latest window and `next_target_ids` gives the next simultaneous bands. `last_job` and `next_target_id` expose the first entries for existing clients.
- `monitoring.status`: `unmonitored`, `partial` or `captured_channels_only`. None asserts full-band or geographic RF coverage.

A failed job or inaccessible source does not add a no-detection observation. `no_detection` requires processed samples and applies only to that detector, channel and interval.

## Result semantics

| Field | Meaning |
|---|---|
| `status=located` | One accepted solution; `estimate` contains centre and contour |
| `status=undetermined` | `estimate=null`; ambiguity may retain distinct `hypotheses` |
| `status=no_detection` | No emission identified by the documented detector in the recorded channel |
| `status=no_verified_beacon` | Synchronized samples cover the IBP slot and no receiver passed the callsign verification |
| `receivers[].confirmed` | An independently verified IBP callsign in that receiver's IQ, or a correlated reception for the other profiles |
| `receivers[].used` | Actually used in a solution/hypothesis calculation |
| `synchronized`, `gps_age_max_s`, `reason` | State checked in the recorded IQ |
| `identity=null` | Display `unknown callsign`; frequency is not identity |
| `pairs[].delay_us` | Arrival B minus arrival A, in microseconds |
| `fault_injected=true` | A replay test with GNSS metadata invalidated in memory; RF unchanged |

The detail route ignores `bbox`, preserving confirmed receivers outside the viewport. Correlation coefficients are not probabilities. Uncertainty contours are model-dependent and not statistically calibrated.

For live DCF77, `reference.profile` identifies a phase code verified against each recorded IQ hash. With four or more contributing receivers, `reference.three_receivers` also lists the three IDs selected by the largest normalized triangle area, with ties broken by stable receiver ID order. Its `solutions` use measured delays from the same capture. An empty array carries a reason when available. Multiple solutions retain all compatible hypotheses. This comparison leaves the full-network result and its identity intact.

## Digital activity

The live map opens with TDoA transmitter positions. `/?station=DCF77` opens the current measurement of that reference station and follows its band. `/?station=CS3B&band=20m` follows the callsign on 20 metres and shows a waiting message when the selected interval contains no confirmed reception on that band. `/?view=activity`, `/?field=KN` and the receiver links enable the reception overlay.

`GET /api/activity` provides the live digital layer independently of IQ results. It includes `enabled`, broker connection state and connection time, the rolling UTC window, the total number of distinct reporting stations and a `bands` array. Each mode/band row has its report count, distinct reporter count, latest UTC time and counts by two-letter Maidenhead field. Reports with a missing or invalid transmitter locator contribute only to totals. A map link such as `/?field=KN` opens a whole field.

## Directory

`GET /api/beacons` serves the five-channel schedule and the worldwide receiver directory. Entries with published coordinates appear in one of two arrays. Each `receiver_pool` entry has `id`, `label`, `position` as `[latitude,longitude]` and its eligible HF beacon `bands`. The `other_receivers` array supplies the first exclusion `reason` and the directory's reported GPS and channel counts. Both describe the directory at `directory_utc`. The live map links to each receiver with `?receiver=<id>`. Transmitter positions come from synchronized observations.

```text
GET /api/sources?offset=0&limit=30&eligible=false&search=PE1NOD
```

At most 200 entries per response. `next_offset: null` ends pagination. Metadata age, API allowances, candidates and exclusions are returned. Directory `reception_confirmed` is always false; only processed IQ can establish reception.

## Jobs and authentication

```json
{"dataset_id":"dcf77-20200813","fault_receiver":null}
```

Send this to `POST /api/replay`. Use `"fault_receiver":"pa0rdt"` for the GNSS-loss replay. Existing results are accessible without a token and return `cached: true`. Any uncached processing requires an administrator token; use `force: true` to explicitly recompute. Paths cannot be supplied by the client.

```http
POST /api/capture
Authorization: Bearer <administrator-token>
Content-Type: application/json

{"target_id":"dcf77-validation","source_ids":["ID1","ID2","ID3"]}
```

`source_ids` is optional. Manual capture uses the target's configured receivers when omitted. IDs resolve to directory entries. A single processing job runs at a time and conflicts return 409. DCF77 requires phase verification on every recorded receiver for automatic processing. Shared IBP captures always go through the slot verifier. Their jobs include `capture_id`, `sampled_slots`, `verified_slots`, `captured_receivers` and verified `emission_ids`. Each verified detail has `reference.cw` with IQ hashes and per-receiver scores. For IBP, `pairs[].windows` counts matched Morse keying edges. A whole shared-channel capture cannot be promoted through `/api/review`.

```http
POST /api/review
Authorization: Bearer <administrator-token>
Content-Type: application/json

{"capture_id":"live-<UUID>","evidence":{"url":"https://<actual-evidence>","note":"How this evidence links this exact recording to an eligible emission"},"identity":null}
```

A separately verified identity can include `callsign` and its own `evidence`. These are operator assertions; the service does not claim to automatically verify the content of external evidence pages.

Jobs are durable: `running`, `completed`, `awaiting_review`, `failed`, `interrupted`. On restart, unfinished jobs become `interrupted`, without automatic retry. `persistence_error` means the final state could not be saved; an HTTP response alone does not establish durability. Captures pending review are rediscovered from disk. The API neither deletes captures nor exposes raw IQ downloads.

Provide a token of at least 24 characters through `MERIDIAN_ADMIN_TOKEN_FILE` or `MERIDIAN_ADMIN_TOKEN`. Compose generates it in `.secrets/runtime.env`; Render generates it through the Blueprint. Use HTTPS for remote administration. Tokens are not returned by the API or persisted by the browser.
