# syntax=docker/dockerfile:1.7

FROM rust:1.94-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends brotli gzip \
    && rm -rf /var/lib/apt/lists/*

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    rustup target add wasm32-unknown-unknown \
    && cargo install trunk --version 0.21.14 --locked

WORKDIR /app
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    cargo build -p mew-image-backend --profile docker-release --locked \
    && cd frontend \
    && trunk build --release --locked --dist dist-app \
    && find dist-app -type f \( \
        -name '*.wasm' -o -name '*.js' -o -name '*.css' -o -name '*.svg' \
    \) -exec brotli --quality=11 --keep {} \; \
    && find dist-app -type f \( \
        -name '*.wasm' -o -name '*.js' -o -name '*.css' -o -name '*.svg' \
    \) -exec gzip -9 --keep {} \; \
    && cd .. \
    && install -D -m 0755 target/docker-release/mew-image-backend \
        /out/bin/mew-image-backend \
    && cp -a frontend/dist-app /out/frontend

FROM debian:bookworm-slim AS certificates

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

FROM debian:bookworm-slim

RUN groupadd --gid 10001 mewimage \
    && useradd --uid 10001 --gid 10001 \
        --create-home --home-dir /home/mewimage \
        --shell /usr/sbin/nologin mewimage \
    && install -d -m 0750 -o 10001 -g 10001 /data /data/assets

WORKDIR /app
COPY --from=certificates /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /out/bin/mew-image-backend /usr/local/bin/mew-image-backend
COPY --from=builder /out/frontend /app/frontend/dist-app

ENV MEW_LISTEN=0.0.0.0:3000
ENV MEW_DATABASE_URL=sqlite:///data/mew-image.db
ENV MEW_FRONTEND_DIST=/app/frontend/dist-app
ENV MEW_ASSET_STORE=local
ENV MEW_LOCAL_ASSET_DIR=/data/assets
# 大型参考图临时文件落在持久卷的受限子目录，避免挤占容器内存型 /tmp。
ENV TMPDIR=/data/.tmp

VOLUME ["/data"]

EXPOSE 3000

USER 10001:10001

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/usr/local/bin/mew-image-backend", "healthcheck"]

STOPSIGNAL SIGTERM

CMD ["mew-image-backend"]
