# syntax=docker/dockerfile:1.7

# ---- Builder ---------------------------------------------------------------
FROM rust:1.91-slim-bookworm AS builder

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_TOOLCHAIN=stable \
    CARGO_TERM_COLOR=always \
    CARGO_HOME=/usr/local/cargo

ARG TARGETARCH

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        gnupg \
        pkg-config \
        libssl-dev \
        clang \
        build-essential \
 && curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
 && apt-get install -y --no-install-recommends nodejs

WORKDIR /build

# Install JS deps first so the npm layer caches across Rust source changes.
# /root/.npm is npm's package download cache; node_modules itself stays in
# the layer because it is read by the bundle step that follows.
COPY railgun-runtime/package.json railgun-runtime/package-lock.json ./railgun-runtime/
RUN --mount=type=cache,target=/root/.npm,sharing=locked,id=npm-cache \
    cd railgun-runtime && npm ci --no-audit --no-fund

# Copy the rest of the source. .dockerignore strips target/, node_modules/,
# embedded/, artifacts/, .arti/, .env, and .git so the build context is small.
COPY . .

# Build the embedded JS bundle. Output lands at /build/embedded/ in the layer.
RUN cd railgun-runtime && npm run bundle:embedded

# Build the Rust binary. Cargo registry, git, and target dir live on cache
# mounts (per-arch ids prevent cross-platform reuse). The binary ends up
# inside the cache-backed target/, so copy it out before the RUN closes.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked,id=cargo-git-${TARGETARCH} \
    --mount=type=cache,target=/build/target,sharing=locked,id=cargo-target-${TARGETARCH} \
    cargo build --release --locked --bin hermetic \
 && cp /build/target/release/hermetic /tmp/hermetic

# ---- Runtime ---------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

ENV DEBIAN_FRONTEND=noninteractive

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates

WORKDIR /app

# The Rust binary, copied out of the cache-mounted target/ in the builder.
COPY --from=builder /tmp/hermetic /usr/local/bin/hermetic

# The embedded Railgun bundle (loaded at runtime by the Deno worker).
COPY --from=builder /build/embedded/railgun_runtime.bundle.mjs /app/embedded/railgun_runtime.bundle.mjs

# WASM addons that the SDK loads at runtime via Node module resolution. Kept
# under the same path the Deno permissions allowlist expects.
COPY --from=builder /build/railgun-runtime/node_modules/@railgun-community/poseidon-hash-wasm \
     /app/railgun-runtime/node_modules/@railgun-community/poseidon-hash-wasm
COPY --from=builder /build/railgun-runtime/node_modules/@railgun-community/curve25519-scalarmult-wasm \
     /app/railgun-runtime/node_modules/@railgun-community/curve25519-scalarmult-wasm

# Persistent state lives under these mountpoints. VOLUME documents the
# contract; users supply named volumes or bind mounts at `docker run` time.
RUN mkdir -p /app/artifacts /app/.arti
VOLUME ["/app/artifacts", "/app/.arti"]

ENTRYPOINT ["hermetic"]
