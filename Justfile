default:
    @just --list

build:
    cargo build --release
    cargo deny check bans
    cargo clippy --all-targets -- -D warnings

check-deps:
    cargo build
    @cargo tree -e features | rg '(reqwest|alloy-transport-http|HttpConnector|hyper-tls|native-tls)' && exit 1 || true
    cargo deny check bans
    cargo fmt --check

spike:
    cargo run --release --example spike
    cargo test --release --test spike_connector_counter

check-static:
    @rg -n --pcre2 '(TcpStream::connect|lookup_host|to_socket_addrs|reqwest|HttpConnector|ProviderBuilder::on_http|fetch\()' src sidecar examples tests | rg -v '^([^:]*):(\s*//|\s*\*)' | rg -v "sidecar/main\\.mjs:.*fetch\\('https://example.com'\\)" && exit 1 || true
    cargo clippy --all-targets -- -D warnings

ping rpc:
    cargo run --release -- ping --rpc {{rpc}}

sidecar-smoke:
    docker build -t undercover-sidecar:dev sidecar
    cargo run --release -- sidecar-smoke

load-wallet-smoke mnemonic:
    docker build -t undercover-sidecar:dev sidecar
    cargo run --release -- load-wallet-smoke --railgun-mnemonic "{{mnemonic}}"

signer-address:
    cargo run --release -- signer-address

check-sidecar:
    docker build -t undercover-sidecar:dev sidecar
    cargo test --test sidecar_permissions_smoke
    cargo run --release -- sidecar-smoke
    cargo run --release -- load-wallet-smoke --railgun-mnemonic "test test test test test test test test test test test junk"

shield-base-token amount_wei rpc="https://ethereum-sepolia-rpc.publicnode.com":
    docker build -t undercover-sidecar:dev sidecar
    cargo run --release -- shield-base-token --rpc {{rpc}} --amount-wei {{amount_wei}}

shield-base-token-dry-run amount_wei rpc="https://ethereum-sepolia-rpc.publicnode.com":
    docker build -t undercover-sidecar:dev sidecar
    cargo run --release -- shield-base-token --dry-run --rpc {{rpc}} --amount-wei {{amount_wei}}

refresh-balance creation_block="0" rpc="https://ethereum-sepolia-rpc.publicnode.com":
    docker build -t undercover-sidecar:dev sidecar
    cargo run --release -- refresh-balance --rpc {{rpc}} --creation-block {{creation_block}}
