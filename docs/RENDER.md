# Deploy on Render

Provider documentation checked on 27 November 2026. This project uses a **Docker Web Service**, containing Rust, Python and KiwiClient. Render provides an `onrender.com` URL and managed HTTPS; no VPS, SSH, custom domain or Caddy is needed for this deployment.

[`render.yaml`](../render.yaml) explicitly selects **Free**, with no paid disk or database. JSON Schema validation checks the file's structure; Render also checks plan-specific restrictions when creating the Blueprint.

## Installation

1. Use the README's **Deploy to Render** button, or choose **New → Blueprint** in Render and connect `s1lviu/meridian-tdoa` (or your fork).
2. Select `render.yaml`; review that the service is **Free**, then create it.
3. Wait for the build and open the URL Render assigns. `/api/ready` should return `status: ready`. Open `/?replay=1` for the bundled real recording.
4. Find the generated `MERIDIAN_ADMIN_TOKEN` in the service's Environment settings. Enter it in the application only when requesting capture or reprocessing. Never commit it or place it in a URL.

The configuration uses port 3000 on `0.0.0.0` and `/api/ready`. It omits `maxShutdownDelaySeconds` because Render Free rejects custom shutdown delays; Render's documented default is 30 seconds. A capture still running at shutdown can be interrupted. Subsequent deploys are manual (`autoDeployTrigger: off`) to avoid interrupting a capture whenever a commit is pushed.

When using another repository, keep the Dockerfile and Blueprint at its root. Include `third_party/kiwiclient-4eb733e6.tar.gz`, the local frontend assets and bundled recordings/results. Exclude secrets and build directories.

### Blueprint validation error

If Render reports `max shutdown delay is not supported for free tier services`, select **Retry** after loading the latest `main` branch. The current Blueprint omits that field and keeps `plan: free`. For a fork, pull the correction or remove the `maxShutdownDelaySeconds` entry from its `render.yaml`.

## Free-plan limitations

Free can serve the map, API and included real recording. The delivered configuration also allows administrator-initiated live capture attempts; actual RF reception and synchronization still need verification from the host.

The service sleeps after 15 minutes without inbound traffic. New IQ, results and job history are lost on restart, redeploy or sleep; bundled historical files are restored from the image. Free has no persistent disks and may be suspended for excessive service-initiated internet traffic. Its 512 MB RAM has not been validated for the full allowed workload. Artificial keep-alive traffic is not used.

This is a testing profile, not a durable production archive.

## Persistent storage

A paid compute plan is required. Published pricing at the review date starts at **USD 7/month for 512 MB**, or **USD 25/month for 2 GB**, plus **USD 0.25/GB/month** for a disk. Validate memory requirements before selecting a size; traffic/build charges and taxes may apply separately.

After explicitly choosing paid resources, this is one example change to the same service:

```yaml
    plan: 1c-2g
    disk:
      name: meridian-data
      mountPath: /app/data
      sizeGB: 5
```

That example totals **USD 26.25/month** for compute and disk at the cited rates. It is not enabled by the Free Blueprint. PostgreSQL and Redis are not required by this implementation. Keep a single instance; disk-backed redeploys can interrupt service.

Check that UID 10001 can write to the mounted path, readiness works, state survives redeploy and backups can be restored. Then follow the [live RF acceptance procedure](PRODUCTION.md#required-acceptance-on-the-chosen-host). Paying for hosting does not validate those measurements.

## Official references

- [Docker](https://render.com/docs/docker), [Web Services](https://render.com/docs/web-services), [Deploy button](https://render.com/docs/deploy-to-render)
- [Blueprint specification](https://render.com/docs/blueprint-spec), [Free limits](https://render.com/docs/free)
- [Persistent disks](https://render.com/docs/disks), [pricing](https://render.com/pricing)
