# syntax=docker/dockerfile:1

# -----------------------------------------------------------------------------
# Stage 1: Planner — generate a locked dependency recipe for cargo-chef.
# -----------------------------------------------------------------------------
FROM rust:1.97 AS planner
WORKDIR /app

RUN cargo install cargo-chef --locked

COPY Cargo.toml Cargo.lock ./
RUN cargo chef prepare --recipe-path recipe.json

# -----------------------------------------------------------------------------
# Stage 2: Builder — cache dependency builds, then compile the application.
# -----------------------------------------------------------------------------
FROM rust:1.97 AS builder
WORKDIR /app

RUN cargo install cargo-chef --locked

# Install Node.js for building Tailwind CSS assets.
# Combine into one RUN to ensure PATH and binaries are properly set up.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl gnupg \
    && curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/* \
    && node --version \
    && npm --version

# Reduce release-build memory usage inside Docker so large crates (surrealdb-core) don't OOM.
# These overrides are intentionally conservative; they trade peak memory for longer compile times.
ENV CARGO_PROFILE_RELEASE_LTO=false
ENV CARGO_PROFILE_RELEASE_CODEGEN_UNITS=64

# Allow overriding parallel job count via --build-arg BUILD_JOBS=N.
# Use "default" to let Cargo pick the core count; set lower (e.g. 1 or 2) on memory-constrained hosts.
ARG BUILD_JOBS=default
ENV CARGO_BUILD_JOBS=${BUILD_JOBS}

# Cache dependency compilation. This layer is invalidated only when Cargo.toml/Cargo.lock change.
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build frontend assets first so css is ready before Rust compilation finishes.
# Touch a marker file to force Docker cache invalidation on each build.
COPY assets ./assets
# Copy templates early so Tailwind's @source can scan them during build.
COPY templates ./templates
RUN cd assets \
    && npm ci --no-audit --no-fund --ignore-scripts \
    && npm run build:css \
    && npm run copy:js \
    && ls -la js/ output.css

# Copy source and build the application binary. This layer is invalidated when src/ changes.
COPY src ./src
COPY schema ./schema
RUN cargo build --release --bin webfind

# -----------------------------------------------------------------------------
# Stage 3: Runtime — minimal image with just the compiled binary.
# -----------------------------------------------------------------------------
FROM ubuntu:24.04 AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/webfind /usr/local/bin/webfind
COPY --from=builder /app/assets ./assets
COPY --from=builder /app/templates ./templates

ENV WEBFIND_DATA_DIR=/data
VOLUME ["/data"]
# Container-internal ports:
#   4747 : API/MCP over HTTP (Streamable HTTP)
#   4749 : HTML GUI (Google-style search)
# When running via docker compose, the host ports are:
#   5748 -> 4747 (API)
#   5750 -> 4749 (GUI)
EXPOSE 4747 4749

ENTRYPOINT ["webfind"]
CMD ["serve", "--transport", "http", "--gui-port", "4749"]
