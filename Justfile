default:
    @just --list

fmt:
    cargo fmt
    deno fmt railgun-runtime/src/ railgun-runtime/build-embedded.mjs src/embedded/bootstrap.js

check:
    cargo fmt --check
    deno fmt --check railgun-runtime/src/ railgun-runtime/build-embedded.mjs src/embedded/bootstrap.js
    deno lint railgun-runtime/src/ railgun-runtime/build-embedded.mjs src/embedded/bootstrap.js
    cargo clippy --all-targets -- -D warnings -W clippy::pedantic
    cargo check
    cargo test

deny:
    cargo deny check bans

bundle:
    cd railgun-runtime && npm run bundle:embedded

doctor: bundle
    cargo run -- doctor

build: check deny doctor
    cargo build --release
