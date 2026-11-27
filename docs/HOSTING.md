# Free hosting options

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
