# Meridian API

JSON over the same origin as the UI. Times use RFC 3339 with an explicit timezone; the UI sends UTC `Z`. Request bodies are limited to 16 KiB. No permissive CORS is enabled.

| Method | Route | Purpose |
|---|---|---|
| GET | `/api/health` | Process health |
| GET | `/api/ready` | Readiness; 503 while stopping, otherwise version and worker state |
| GET | `/api/info` | Public configuration, datasets and pending captures; no token |
| GET | `/api/sources` | Paginated public directory and exclusion reasons |
| GET | `/api/emissions` | Geographic, temporal, band and origin filtering |
| GET | `/api/emissions/{id}` | Full result, all receiver observations and evidence |
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
- `band`: `all`, `lf`, `mf`, `hf`, `80m`, `40m`, `20m`. These are display filters, not regional transmitting authorizations.
- `start`, `end`: half-open interval, start < end.
- `mode`: `replay` (default) or `live`.
- `scenario`: `normal` or `sync-loss`; injected faults are separate from normal results.

Response fields:

- `emissions`: estimated regions/hypotheses intersecting the viewport. A contour does not imply a unique position; inspect `status`.
- `unlocated`: undetermined observations without contours. They cannot be filtered geographically and are not plotted at a receiver or map centre.
- `total_matching`: detected emissions matching the non-geographic filters.
- `monitoring.segments`: captured intervals and channels, independent of the viewport.
- `monitoring.gaps`: the temporal complement of recorded segments in the selected interval.
- `monitoring.status`: `unmonitored`, `partial` or `captured_channels_only`. None asserts full-band or geographic RF coverage.

A failed job or inaccessible source does not add a no-detection observation. `no_detection` requires processed samples and applies only to that detector, channel and interval.

## Result semantics

| Field | Meaning |
|---|---|
| `status=located` | One accepted solution; `estimate` contains centre and contour |
| `status=undetermined` | `estimate=null`; ambiguity may retain distinct `hypotheses` |
| `status=no_detection` | No emission identified by the documented detector in the recorded channel |
| `receivers[].confirmed` | Accepted RF correlation with at least one other receiver |
| `receivers[].used` | Actually used in a solution/hypothesis calculation |
| `synchronized`, `gps_age_max_s`, `reason` | State checked in the recorded IQ |
| `identity=null` | Display `unknown callsign`; frequency is not identity |
| `pairs[].delay_us` | Arrival B minus arrival A, in microseconds |
| `fault_injected=true` | A replay test with GNSS metadata invalidated in memory; RF unchanged |

The detail route ignores `bbox`, preserving confirmed receivers outside the viewport. Correlation coefficients are not probabilities. Uncertainty contours are model-dependent and not statistically calibrated.

## Directory

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

`source_ids` is optional; the target's configured receivers are used when omitted. IDs are resolved from the directory, not arbitrary client URLs. A single processing job runs at a time; conflicting work returns 409. Successful capture proceeds automatically only if the configured DCF77 RF profile verifies every recorded receiver. Otherwise it remains `awaiting_review`.

```http
POST /api/review
Authorization: Bearer <administrator-token>
Content-Type: application/json

{"capture_id":"live-<UUID>","evidence":{"url":"https://<actual-evidence>","note":"How this evidence links this exact recording to an eligible emission"},"identity":null}
```

A separately verified identity can include `callsign` and its own `evidence`. These are operator assertions; the service does not claim to automatically verify the content of external evidence pages.

Jobs are durable: `running`, `completed`, `awaiting_review`, `failed`, `interrupted`. On restart, unfinished jobs become `interrupted`, without automatic retry. `persistence_error` means the final state could not be saved; an HTTP response alone does not establish durability. Captures pending review are rediscovered from disk. The API neither deletes captures nor exposes raw IQ downloads.

Provide a token of at least 24 characters through `MERIDIAN_ADMIN_TOKEN_FILE` or `MERIDIAN_ADMIN_TOKEN`. Compose generates it in `.secrets/runtime.env`; Render generates it through the Blueprint. Use HTTPS for remote administration. Tokens are not returned by the API or persisted by the browser.
