default:
    @just --list

fmt:
    cargo fmt
    deno fmt railgun-runtime/runtime.mjs src/hermetic_host_ops.js

check:
    cargo fmt --check
    deno fmt --check railgun-runtime/runtime.mjs src/hermetic_host_ops.js
    deno lint railgun-runtime/runtime.mjs src/hermetic_host_ops.js
    cargo clippy --all-targets -- -D warnings -W clippy::pedantic
    just static
    cargo check
    cargo test

static:
    @cargo tree -e features | rg '(reqwest|alloy-transport-http|HttpConnector|hyper-tls|native-tls)' && exit 1 || true
    @rg -n '(TcpStream::connect|lookup_host|to_socket_addrs|reqwest|HttpConnector|ProviderBuilder::on_http|fetch\()' src railgun-runtime | rg -v '^([^:]*):(\s*//|\s*\*)' | rg -v 'DENIED_FETCH_PROBE_URL' && exit 1 || true

deny:
    cargo deny check bans

bundle:
    cd railgun-runtime && npm run bundle:embedded

doctor: bundle
    cargo run -- doctor

build: check deny doctor
    cargo build --release
