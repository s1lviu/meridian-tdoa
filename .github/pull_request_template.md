## What changed

## How it was verified

<!-- Commands and their results, e.g. scripts/check.sh. Every measured number needs an evidence file. -->

- [ ] `scripts/check.sh` passes, or the subset run is named above
- [ ] Positions still come only from our own timing measurements (measurement contract in CONTRIBUTING.md)
- [ ] Docs, `docs/API.md` and evidence updated where behaviour or numbers changed

Merging deploys `main` to Render once checks pass, and a deploy clears the data collected on the Free plan.
