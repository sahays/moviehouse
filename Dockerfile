# syntax=docker/dockerfile:1

# ── Stage 1: build ────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

# Node is required at build time: build.rs runs `npm install && npm run build`
# in frontend/ and rust-embed bakes the compiled SPA into the binary.
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - && \
    apt-get install -y --no-install-recommends nodejs && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Full source is needed because build.rs compiles the frontend during the build.
COPY . .

# Release build (build.rs builds the frontend and bumps the patch version in an
# ephemeral copy of Cargo.toml — harmless in the throwaway build stage).
RUN cargo build --release

# ── Stage 2: runtime ──────────────────────────────────────────────
FROM debian:bookworm-slim

# ffmpeg for transcoding; curl for the container healthcheck.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        ffmpeg && \
    rm -rf /var/lib/apt/lists/*

# Unprivileged runtime user. HOME=/data so the app's $HOME-derived paths
# (~/.movies/data, ~/.moviehouse/dht_nodes.json) land on the mounted data volume.
RUN groupadd --system app && \
    useradd --system --gid app --home-dir /data app && \
    mkdir -p /data /media/downloads /media/transcoded && \
    chown -R app:app /data /media

COPY --from=builder /app/target/release/moviehouse /app/bin/moviehouse

ENV HOME=/data

# Media lives on the /media bind mount, not on $HOME (the data volume holds only
# the sled db). WORKDIR is root-owned and the process runs as `app`, so these
# must be set: a relative or $HOME-derived default would be the wrong disk at
# best and unwritable at worst.
ENV MOVIEHOUSE_DOWNLOAD_DIR=/media/downloads \
    MOVIEHOUSE_TRANSCODE_DIR=/media/transcoded

WORKDIR /app
USER app

EXPOSE 9000 6881

# Shell form so ${WEB_PORT} is expanded from the container environment
# (provided by env_file in docker-compose). exec keeps the binary as PID 1.
# `serve` takes its output directory from settings (seeded by the env vars
# above), not from the global --output flag, which applies to the CLI
# download/magnet subcommands only.
CMD ["sh", "-c", "exec /app/bin/moviehouse serve --bind 0.0.0.0:${WEB_PORT:-9000} --port 6881"]
