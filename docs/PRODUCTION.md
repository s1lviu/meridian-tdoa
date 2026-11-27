# Deployment and operation

The native instance acquired and localized the live DCF77 time signal on four receivers on 27 November 2026. Each hosting instance needs its own transport and storage checks. [Render setup](RENDER.md) and [hosting alternatives](HOSTING.md) describe deployment.

## Docker on a server

The supplied `deploy/cloud-init.yaml` installs Docker on Ubuntu 24.04. On a server you administer:

1. Allow inbound TCP 80/443 and restrict SSH to administrator addresses.
2. Point a domain's DNS A record at the server. Caddy requests TLS certificates after DNS resolves correctly.
3. Clone this repository into a directory owned by the deployment user and run:

```sh
./scripts/deploy.sh your-domain.example
```

The script creates `.secrets/runtime.env` with restricted permissions and an administrator token, builds the image, starts Compose and checks HTTPS readiness. Do not publish the token in Git or a URL. The public map is read-only; capture, uncached reprocessing, review and manual directory refresh require the token.

Compose uses UID 10001, a read-only root filesystem, a persistent data volume, rotated logs, readiness checks, resource limits and graceful shutdown. Caddy terminates TLS; port 3000 is bound only to localhost. This architecture supports one application instance writing to the archive.

Validate resource enforcement on the actual host. In the managed development Docker environment, a requested 1 GiB memory/128-process limit was not applied: the cgroup reported 16 GiB and unlimited processes. Non-root operation, a read-only filesystem and readiness were verified. See `evidence/deployment-checks.json`. The pinned Rust/Python base manifests include Linux ARM64, but no ARM hosting acceptance run has been completed.

## Live configuration

`config/production.json` includes a DCF77 commissioning target and the five NCDXF/IARU beacon frequencies. Its fixed receiver list serves DCF77 and manual captures. The HF monitor selects from the worldwide directory at each attempt.

- `permission.basis: published_api`: recheck positive `ext_api` and `tdoa_ch` directly on every capture, together with band, availability, GNSS and antenna metadata. A published allowance is not a reserved channel. Zero or missing permission is not accepted.
- Other permission bases require documented authorization and a future `expires_utc`; expired authorizations fail closed. `expires_utc` is not used as an invented operator promise for `published_api`.
- Positions are operator-reported, associated with antenna metadata and compared against `/status`. The 1 km allowance is a conservative assumption, not an independent phase-centre survey. Antenna/cable delays are not calibrated.
- `gps_offset_valid_until_utc` requires a fresh GPS–UTC offset review. The bundled 18 s offset follows IERS Bulletin C72, with a conservative revalidation deadline of **1 January 2027**; do not extend it without checking the current bulletin.
- `verification: dcf77_prn_v1` matches the PTB PRN code on every saved receiver: at least four 2 s windows, normalized correlation ≥0.65 in ≥80% of windows, at least three receivers. Frequency alone never verifies an emission. Automatic live identity remains unknown.
- `verification: ncdxf_cw_v1` processes ten-second GNSS-aligned transmissions independently. Each receiver must match the complete scheduled Morse callsign and first power dash. A single verified receiver can establish reception. Position requires independent receivers, accepted differential delays and a bounded solution. [Controller timing](https://www.ncdxf.org/beacon/beaconcontroller.html)
- Other documented public beacons or participating amateur stations use manual capture-specific RF review.

### Scheduled public observatory

The production profile runs one worker for all visitors. The same settings work in a native live configuration.

```json
"monitor": {
  "target_id": "dcf77-validation",
  "additional_target_ids": ["ibp-20m", "ibp-17m", "ibp-15m", "ibp-12m", "ibp-10m"],
  "interval_s": 73,
  "directory_receivers": 8,
  "parallel_bands": 2
}
```

Every 73 seconds the worker schedules two of the six targets for simultaneous 30-second IQ captures. The rotation advances by the number of scheduled targets, covering all six in three windows. Each target gets its own job, manifest, UTC overlap and receiver set. Fixed LF receivers take precedence when LF is scheduled. The HF pool rotates through eligible locations, and each receiver has one assignment per window. The group limit of eight bounds the solver's exhaustive subset search. The worker reserves the combined archive allowance and cooldown before opening receivers. `parallel_bands` defaults to one in older configurations and may be set up to the number of configured monitor targets.

The production profile also sets `digital_reports` to true. One independent server connection receives FT8 and WSPR reports over TLS from `mqtt.pskreporter.info:1884` with exponential reconnect delays. The server retains counters for the current UTC minute and the preceding 14 minutes in memory. Reports expire during outages and collection resumes on reconnect. Digital reports run independently of the IQ archive quota and scheduler. [Feed and attribution](https://www.mqtt.pskreporter.info/)

The world view distinguishes the current schedule, measured receptions and TDoA fixes. Visitors can open any of the 18 beacon entries and inspect its verified bands in the chosen interval. The page reads shared results every eight seconds. No receiver connections are opened by a visitor browsing the map.

Omit `additional_target_ids` for a single target. Set `directory_receivers` to zero to use configured receiver IDs. Omit `monitor` for administrator-triggered captures only. Supported intervals range from 60 to 86,400 seconds. Scheduled and manual captures share one worker and the on-disk cooldown. Missed windows are skipped. A full archive, persistence failure or expired GPS–UTC validation pauses scheduling. Each receiver's permission is checked at acquisition time. Directory refresh is cached for at least 30 minutes.

## Required acceptance on the chosen host

The earlier container run could retrieve `/status` but failed to acquire IQ. The native run subsequently captured synchronized IQ. Test from the actual deployment host because a successful metadata response establishes only reachability of that endpoint.

Before declaring the deployment operational for live measurements:

1. Capture an eligible public reference simultaneously on at least three independent receivers. Retain source checks, original IQ, hashes, capture outcomes and RF eligibility verification.
2. Verify common UTC overlap, synchronization in IQ blocks, correlation, geometry and browser results. Ambiguity remains **Position undetermined**.
3. Test lost synchronization on a separately labelled replay; never mutate the original capture or call a fault injection live evidence.
4. Evaluate the known reference position only after localization. Do not use it to choose a solution or search region.
5. Verify resource limits, restart recovery and backup restoration on that hosting instance.

## Storage, backup and recovery

All mutable state is under `/app/data` in Docker. A disk must cover this entire path, including IQ, manifests, results, jobs and directory state. Unfinished jobs become `interrupted` after a forced stop and are not retried. State is written with fsync and atomic rename. Missing intervals never become no-detection observations.

For Compose deployments:

```sh
./scripts/backup.sh /path/to/meridian-backup.tar.gz
```

The script stops the worker, saves a consistent volume archive, verifies it and restarts the worker. Store a copy outside the instance. Keep configuration and the administrator secret separately. To restore, stop the app, restore the verified archive with UID/GID 10001, start it and check readiness, jobs and results. Never use `docker compose down -v` for an update: it deletes volumes.

The IQ archive quota defaults to 2 GiB (`MERIDIAN_CAPTURE_QUOTA_BYTES`). Each attempt conservatively reserves up to 64 MiB per receiver. Captures are refused before quota exhaustion; no automatic deletion occurs. At 10,000 jobs, archival is required. Multi-instance deployment requires shared storage and worker coordination and is not supported by the current single-writer design.

For updates, keep volumes and secrets, review source policies and the UTC-offset deadline, run the checks, then `docker compose up -d --build --wait`. Preserve a verified backup and the previous image digest for rollback.
