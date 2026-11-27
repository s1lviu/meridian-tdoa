# Deployment and operation

For the simplest Git-based installation, use [Render](RENDER.md). [Hosting alternatives](HOSTING.md) evaluates other services. The source repository and local checks are available; a public application deployment and successful live RF acquisition have not yet been demonstrated.

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

`config/production.json` contains five real receiver candidates checked on 26 November 2026 and a DCF77 commissioning target. These observations do not guarantee future availability. The UI can select another 3–8 directory receivers for a configured eligible target.

- `permission.basis: published_api`: recheck positive `ext_api` and `tdoa_ch` directly on every capture, together with band, availability, GNSS and antenna metadata. A published allowance is not a reserved channel. Zero or missing permission is not accepted.
- Other permission bases require documented authorization and a future `expires_utc`; expired authorizations fail closed. `expires_utc` is not used as an invented operator promise for `published_api`.
- Positions are operator-reported, associated with antenna metadata and compared against `/status`. The 1 km allowance is a conservative assumption, not an independent phase-centre survey. Antenna/cable delays are not calibrated.
- `gps_offset_valid_until_utc` requires a fresh GPS–UTC offset review. The bundled 18 s offset follows IERS Bulletin C72, with a conservative revalidation deadline of **1 January 2027**; do not extend it without checking the current bulletin.
- `verification: dcf77_prn_v1` matches the PTB PRN code on every saved receiver: at least four 2 s windows, normalized correlation ≥0.65 in ≥80% of windows, at least three receivers. Frequency alone never verifies an emission. Automatic live identity remains unknown.
- Other public beacons or participating amateur stations require documented eligibility and manual capture-specific RF review. There is no universal emission classifier.

Capture is on demand: one job, at most 8 receivers, no automatic retries, at least 60 s between attempts. Source-specific duration limits apply. Directory refresh is cached for at least 30 minutes and does not occupy SDR channels. The UI's live refresh fetches existing results; it does not start captures or imply continuous global monitoring.

## Required acceptance on the chosen host

Live IQ transport failed in the development environment: direct connections were refused, an HTTP CONNECT tunnel closed during handshake, and a proxy WebSocket Upgrade returned HTTP 403. `/status` worked. The exact failing layer was not conclusively identified. Do not infer RF reception from successful metadata requests.

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
