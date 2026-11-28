# Hosting

## Own server behind Apache

For a host that already serves other sites with Apache and certbot. Meridian runs as one container bound to `127.0.0.1`, and Apache proxies the public hostname to it.

The public map uses `https://meridian.145500.xyz/` as its canonical address. For an independent public installation, replace this origin in `web/index.html`, `web/robots.txt` and `web/sitemap.xml` before building the image.

Prepare the host once, as root:

```sh
sudo DOMAIN=meridian.example.org sh deploy/host/setup.sh   # PORT=8790 and DATA_SIZE=10G by default
```

| Created | Where | Purpose |
|---|---|---|
| Releases, compose settings, administrator token (mode 0600) | `/opt/meridian` | Owned by the deploy user |
| Fixed-size ext4 filesystem (default 10 GiB), mounted at boot | `/srv/meridian/data.img` on `/srv/meridian/data` | Meridian's data can never fill the host disk. Without the mount the container refuses to start instead of writing to the root disk |
| Apache site on port 80 | `/etc/apache2/sites-available/DOMAIN.conf` | Proxies everything except `/.well-known/acme-challenge/`, and never redirects |
| Certificate and port 443 site | certbot webroot, `DOMAIN-le-ssl.conf` | Lets Cloudflare use Full (strict) for this hostname |

Every Apache change passes `apache2ctl configtest` before a graceful reload. A rejected site is disabled again, so the other sites keep running.

Deploy a commit from a workstation:

```sh
scripts/deploy-host.sh SSH_HOST [GIT_REF]
```

Only committed files are shipped.

1. The host builds the image in its own BuildKit builder, limited to three cores and 6 GiB, with its cache capped at 8 GB.
2. It starts the image with `deploy/host/compose.yaml`, then waits for the container health check and `/api/ready`.
3. If the check fails, it restarts the previous image.
4. It keeps the three newest releases and their images.

Limits of the running service (`deploy/host/compose.yaml`):

- **CPU and memory:**
  - 3 GiB of memory without swap (`MERIDIAN_MEMORY`);
  - 2 CPUs (`MERIDIAN_CPUS`) at half the default CPU weight;
  - `oom_score_adj` 500, so it is the first candidate if the host runs out of memory.
- **Processes and files:** 256 processes and 8192 open files.
- **Isolation:** read-only root filesystem, no capabilities, `no-new-privileges`, and a 256 MiB `/tmp` in memory.
- **Disk:**
  - 2 GiB IQ quota inside the data filesystem (`MERIDIAN_CAPTURE_QUOTA_BYTES`);
  - container logs capped at 3 × 10 MB;
  - Apache logs rotated daily by logrotate.

### Cloudflare

When the hostname is proxied and the zone uses the **Flexible** SSL mode, Cloudflare connects to port 80 over HTTP. The port 80 site serves the application, and Cloudflare redirects visitors to HTTPS.

Once certbot has issued the certificate, a configuration rule can set **Full (strict)** for this hostname only, which also encrypts the traffic between Cloudflare and the server. Changing the zone-wide mode would affect every other hostname in the zone.

Server-sent events pass through Cloudflare and Apache. The application sends a keep-alive comment every 15 s, so proxies with idle timeouts keep the stream open.

### Operations on the host

```sh
cd /opt/meridian && docker compose --env-file .env --env-file image.env logs -f app
docker stats meridian-app-1      # memory and CPU against the limits
df -h /srv/meridian/data         # data filesystem
```

The administrator token is in `/opt/meridian/.secrets/runtime.env`.

Run the live monitor on one instance only. Each instance keeps its own fair-use budget, so two instances double the load on public receivers.

## Free hosting options

Reviewed on **27 November 2026**. This comparison concerns the existing Rust + Python Docker application, outbound KiwiSDR WebSockets and a durable IQ archive. Provider documentation is evidence of product terms, not a successful RF acceptance test. No provider listed here has been deployed or tested for live capture by this project.

| Service | Relevant free offer | Fit for Meridian |
|---|---|---|
| **Render Free** | Docker, HTTPS and a public URL; 512 MB; idle shutdown after 15 minutes; no persistent disk | Simplest prepared deployment: the repository includes a Free Blueprint. Suitable for testing; newly acquired data is ephemeral. |
| **Northflank Sandbox** | Public pricing advertises 2 services, 2 jobs, 1 database, up to USD 20/month of included resources and no sleeping | The most promising alternative for an always-running trial. Docker is supported. Confirm account-specific compute, volume and network allowances before deployment. |
| **Koyeb Free** | One 512 MB / 0.1 vCPU instance; sleeps after one idle hour; no volumes | Longer idle period than Render, but does not solve persistent IQ storage. |
| **Hugging Face Docker Spaces** | CPU Basic hardware has no hourly charge, but creating Docker Spaces currently requires a paid subscription | Not a strictly free alternative. Outbound ports are limited to 80, 443 and 8080, which excludes the configured Kiwi endpoints on 8073. |

## Recommendation

Use **Render** for the quickest reproducible trial from this repository. **Northflank Sandbox** is worth considering when avoiding idle shutdown matters more: its public offer explicitly says services stay on.

Northflank's current pricing page requires a valid payment method for abuse prevention; an older indexed version says no card, so do not rely on that older claim. Its feature table includes volumes, but the page also lists persistent storage separately at USD 0.11657/GiB/month. These statements do not establish the exact zero-cost disk allocation available in a particular account. Inspect the Sandbox quota and final resource estimate before creating a volume or switching plans. Northflank's documentation also describes Sandbox as a development/hobby tier rather than a production service.

A new free database does not make this filesystem-based application durable. It would require a separate storage implementation, including the raw IQ files. No completely free, production-validated replacement with durable capture storage has been established here.

## Trying Northflank with this repository

This is a documented manual configuration, not an invented deployment API or an already-tested template:

1. Select the Developer Sandbox project and connect this GitHub repository.
2. Build and deploy the root `Dockerfile`, with one service instance.
3. Expose HTTP port **3000** and configure `/api/ready` as the health endpoint.
4. Set `MERIDIAN_CONFIG=/app/config/production.json` and a secret `MERIDIAN_ADMIN_TOKEN` containing at least 24 random characters. Do not store it in Git.
5. Verify the final price and quotas. To retain captures, a persistent volume must cover `/app/data` and be writable by UID 10001. Without it, treat captures and jobs as ephemeral.
6. Open the assigned HTTPS URL and validate the real replay, a bounded live capture and restart persistence. Follow [production acceptance](PRODUCTION.md#required-acceptance-on-the-chosen-host).

## Primary sources

- Render: [Free limits](https://render.com/docs/free), [Docker](https://render.com/docs/docker)
- Northflank: [pricing](https://northflank.com/pricing), [Sandbox terms](https://northflank.com/docs/v1/application/billing/pricing-on-northflank), [account quotas](https://northflank.com/docs/v1/application/billing/resource-plans-and-quotas), [build and deploy](https://northflank.com/docs/v1/application/getting-started/build-and-deploy-your-code)
- Koyeb: [instances](https://www.koyeb.com/docs/reference/instances), [volumes](https://www.koyeb.com/docs/reference/volumes)
- Hugging Face: [Spaces overview, subscriptions and network ports](https://huggingface.co/docs/hub/spaces-overview)
