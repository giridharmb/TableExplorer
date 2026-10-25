# syntax=docker/dockerfile:1
# Table Explorer — multi-stage build: Rust (server + WASM) builder → slim runtime.
#   docker build -t table-explorer .
#   docker run -p 3000:3000 -e DATABASE_URL=... -e STORAGE_DATABASE_URL=... table-explorer
# See docs/DOCKER_DEPLOYMENT.md.

########## builder ##########
FROM rust:1-bookworm AS builder

# wasm target for the browser bundle; cargo-leptos drives both builds (pinned for reproducibility).
RUN rustup target add wasm32-unknown-unknown \
 && cargo install cargo-leptos --version 0.3.11 --locked

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY style ./style

# Produces target/release/webui and target/site/{pkg/webui.js,webui_bg.wasm,webui.css}.
RUN cargo leptos build --release

########## runtime ##########
FROM debian:bookworm-slim AS runtime

# ca-certificates: HTTPS to BigQuery / Google OAuth. Non-root user for the server.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /app app

WORKDIR /app
COPY --from=builder /app/target/release/webui /app/webui
COPY --from=builder /app/target/site /app/site
COPY config /app/config

# Leptos reads these at runtime instead of Cargo.toml metadata.
ENV LEPTOS_OUTPUT_NAME=webui \
    LEPTOS_SITE_ROOT=site \
    LEPTOS_SITE_PKG_DIR=pkg \
    LEPTOS_SITE_ADDR=0.0.0.0:3000 \
    LEPTOS_RELOAD_PORT=3001 \
    LEPTOS_ENV=PROD \
    APP_CONFIG=/app/config/app.toml \
    RUST_LOG=info

USER app
EXPOSE 3000
CMD ["/app/webui"]
