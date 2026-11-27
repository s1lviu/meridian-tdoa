# syntax=docker/dockerfile:1
FROM rust:1.100.0-slim-bookworm@sha256:bb875b0f8ce726053e259034848c471840e96bfe591fd52556178ae9a05e6126 AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock build.rs ./
COPY third_party/ft8_lib ./third_party/ft8_lib
COPY src ./src
COPY web/dcf77-reference.json web/ibp-evaluation.json ./web/
RUN --mount=type=secret,id=proxy_ca \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    if [ -s /run/secrets/proxy_ca ]; then export CARGO_HTTP_CAINFO=/run/secrets/proxy_ca; fi; \
    cargo build --release --locked \
    && cp target/release/meridian-tdoa target/release/meridian-node /usr/local/bin/

FROM python:3.13.9-slim-bookworm@sha256:b685a4fa58bb19d1814d78a1ec0f0208f351452724f78b20212c984d6e124a34 AS runtime
WORKDIR /app
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 OPENBLAS_NUM_THREADS=1 TOKIO_WORKER_THREADS=2 \
    MERIDIAN_CONFIG=/app/config/production.json PORT=3000
RUN --mount=type=secret,id=proxy_ca \
    if [ -s /run/secrets/proxy_ca ]; then export PIP_CERT=/run/secrets/proxy_ca; fi; \
    pip install --no-cache-dir numpy==2.3.5
COPY third_party/kiwiclient-4eb733e6.tar.gz /tmp/kiwiclient.tar.gz
RUN printf 'c23f94ca325826cb8a0ba5e17f2a974c1318b05726070d5070e8a3b399958cc0  /tmp/kiwiclient.tar.gz\n' | sha256sum -c - \
    && mkdir -p vendor/kiwiclient \
    && tar -xzf /tmp/kiwiclient.tar.gz -C vendor/kiwiclient \
    && rm /tmp/kiwiclient.tar.gz \
    && groupadd -g 10001 meridian && useradd -u 10001 -g meridian -M meridian \
    && mkdir -p data && chown 10001:10001 data
COPY --from=build /usr/local/bin/meridian-tdoa /usr/local/bin/meridian-node /usr/local/bin/
COPY web ./web
COPY config ./config
COPY scripts/kiwi_transport.py scripts/container-entrypoint.sh ./scripts/
COPY data/recordings /opt/meridian-seed/recordings
COPY data/results/dcf77-20200813.json data/results/dcf77-20200813-sync-loss.json /opt/meridian-seed/results/
COPY LICENSE THIRD_PARTY.md ./
RUN chmod -R a+rX /app /opt/meridian-seed
USER 10001:10001
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=3s --start-period=20s --retries=3 \
    CMD python3 -c "import urllib.request; urllib.request.urlopen('http://127.0.0.1:3000/api/ready',timeout=2)"
ENTRYPOINT ["/bin/sh", "/app/scripts/container-entrypoint.sh"]
CMD ["meridian-tdoa"]
