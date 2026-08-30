FROM rust:1.94-bookworm AS builder

RUN rustup target add wasm32-unknown-unknown && cargo install trunk

WORKDIR /app
COPY . .

RUN cargo build -p mew-image-backend --profile docker-release
RUN cd frontend && trunk build --release --dist dist-app

FROM debian:bookworm-slim AS certificates

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

FROM debian:bookworm-slim

WORKDIR /app
COPY --from=certificates /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /app/target/docker-release/mew-image-backend /usr/local/bin/mew-image-backend
COPY --from=builder /app/frontend/dist-app /app/frontend/dist-app

ENV MEW_LISTEN=0.0.0.0:3000
ENV MEW_DATABASE_URL=sqlite:///data/mew-image.db
ENV MEW_FRONTEND_DIST=/app/frontend/dist-app
ENV MEW_ASSET_STORE=local
ENV MEW_LOCAL_ASSET_DIR=/data/assets

VOLUME ["/data"]

EXPOSE 3000

CMD ["mew-image-backend"]
