# syntax=docker/dockerfile:1.7

# ---- Builder ---------------------------------------------------------------
FROM rust:1.91-slim-bookworm AS builder

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_TOOLCHAIN=stable \
    CARGO_TERM_COLOR=always

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        gnupg \
        pkg-config \
        libssl-dev \
        build-essential \
 && curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
 && apt-get install -y --no-install-recommends nodejs \
 && apt-get clean \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Install JS dependencies first to maximize layer cache hits across source-only changes.
COPY railgun-runtime/package.json railgun-runtime/package-lock.json ./railgun-runtime/
RUN cd railgun-runtime && npm ci --no-audit --no-fund

# Copy the rest of the source. The .dockerignore strips target/, node_modules/,
# embedded/, artifacts/, .arti/, .env, and .git so the context stays small.
COPY . .

# Build the embedded JS bundle, then the Rust binary in release mode.
RUN cd railgun-runtime && npm run bundle:embedded
RUN cargo build --release --locked --bin hermetic

# ---- Runtime ---------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && apt-get clean \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# The Rust binary.
COPY --from=builder /build/target/release/hermetic /usr/local/bin/hermetic

# The embedded Railgun bundle (loaded at runtime by the Deno worker).
COPY --from=builder /build/embedded/railgun_runtime.bundle.mjs /app/embedded/railgun_runtime.bundle.mjs

# WASM addons that the SDK loads at runtime via Node module resolution.
# Kept under the same path the Deno permissions allowlist expects.
COPY --from=builder /build/railgun-runtime/node_modules/@railgun-community/poseidon-hash-wasm \
     /app/railgun-runtime/node_modules/@railgun-community/poseidon-hash-wasm
COPY --from=builder /build/railgun-runtime/node_modules/@railgun-community/curve25519-scalarmult-wasm \
     /app/railgun-runtime/node_modules/@railgun-community/curve25519-scalarmult-wasm

# Persistent state lives under these mountpoints. Declaring VOLUME documents
# the contract; users supply named volumes or bind mounts at `docker run` time.
RUN mkdir -p /app/artifacts /app/.arti
VOLUME ["/app/artifacts", "/app/.arti"]

ENTRYPOINT ["hermetic"]
