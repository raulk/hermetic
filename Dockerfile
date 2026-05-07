# syntax=docker/dockerfile:1.7

# ---- Base toolchain --------------------------------------------------------
# Shared base for every builder stage. Installs apt deps, Node, mold, and
# cargo-chef + sccache so each derived stage starts from the same toolchain.
FROM rust:1.91-slim-bookworm AS chef

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_TOOLCHAIN=stable \
    CARGO_TERM_COLOR=always \
    CARGO_HOME=/usr/local/cargo

ARG TARGETARCH

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates curl gnupg pkg-config libssl-dev \
        clang mold build-essential \
 && curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
 && apt-get install -y --no-install-recommends nodejs

# Install cargo-chef (deps/source split) and sccache (rustc invocation
# cache). These are installed without RUSTC_WRAPPER set so sccache itself
# can build; the wrapper is enabled after this layer for downstream stages.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked,id=cargo-git-${TARGETARCH} \
    cargo install cargo-chef --locked \
 && cargo install sccache --locked

# Tooling settings that apply to every downstream Rust build:
#   * sccache wraps rustc so individual compilation units cache via cache mount.
#   * mold replaces ld for link-time speedup on linux/amd64 and linux/arm64.
ENV RUSTC_WRAPPER=sccache \
    SCCACHE_DIR=/sccache \
    SCCACHE_CACHE_SIZE=10G \
    RUSTFLAGS="-C link-arg=-fuse-ld=mold"

WORKDIR /build

# ---- Recipe planner --------------------------------------------------------
# Extracts a recipe.json describing the dependency graph. The recipe is
# byte-stable across source-only changes, so downstream stages stay cached
# when only src/ moves.
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

# ---- Dependency build ------------------------------------------------------
# Compiles the entire dependency graph with cargo-chef. Layer cache key is
# the recipe.json content (so it survives src/ changes). The cache mount on
# /build/target lets Cargo's incremental compilation reuse compiled artifacts
# across builds: a single dependency upgrade only recompiles that crate plus
# its reverse-dependents, not the whole graph.
FROM chef AS deps-builder
COPY --from=planner /build/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked,id=cargo-git-${TARGETARCH} \
    --mount=type=cache,target=/build/target,sharing=locked,id=cargo-target-${TARGETARCH} \
    --mount=type=cache,target=/sccache,sharing=locked,id=sccache-${TARGETARCH} \
    cargo chef cook --release --recipe-path recipe.json \
 && sccache --show-stats

# ---- JS bundle build -------------------------------------------------------
# Independent of the Rust build: a transient npm flake here doesn't cost the
# dependency compile.
FROM chef AS bundle-builder
COPY railgun-runtime/package.json railgun-runtime/package-lock.json ./railgun-runtime/
RUN --mount=type=cache,target=/root/.npm,sharing=locked,id=npm-cache \
    cd railgun-runtime && npm ci --no-audit --no-fund
COPY railgun-runtime ./railgun-runtime
RUN cd railgun-runtime && npm run bundle:embedded

# ---- Final builder ---------------------------------------------------------
# Inherits from deps-builder so the cooked dependency graph is already
# present. Only this stage rebuilds when src/ changes.
FROM deps-builder AS builder
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY --from=bundle-builder /build/embedded /build/embedded
COPY --from=bundle-builder /build/railgun-runtime/node_modules /build/railgun-runtime/node_modules
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked,id=cargo-git-${TARGETARCH} \
    --mount=type=cache,target=/build/target,sharing=locked,id=cargo-target-${TARGETARCH} \
    --mount=type=cache,target=/sccache,sharing=locked,id=sccache-${TARGETARCH} \
    cargo build --release --locked --bin hermetic \
 && cp /build/target/release/hermetic /tmp/hermetic \
 && sccache --show-stats

# ---- Runtime ---------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

ENV DEBIAN_FRONTEND=noninteractive

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates

WORKDIR /app

COPY --from=builder /tmp/hermetic /usr/local/bin/hermetic
COPY --from=builder /build/embedded/railgun_runtime.bundle.mjs /app/embedded/railgun_runtime.bundle.mjs
COPY --from=builder /build/railgun-runtime/node_modules/@railgun-community/poseidon-hash-wasm \
     /app/railgun-runtime/node_modules/@railgun-community/poseidon-hash-wasm
COPY --from=builder /build/railgun-runtime/node_modules/@railgun-community/curve25519-scalarmult-wasm \
     /app/railgun-runtime/node_modules/@railgun-community/curve25519-scalarmult-wasm

RUN mkdir -p /app/artifacts /app/.arti
VOLUME ["/app/artifacts", "/app/.arti"]

ENTRYPOINT ["hermetic"]
