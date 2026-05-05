default:
    @just --list

build:
    cargo build --release
    cargo deny check bans
    cargo clippy --all-targets -- -D warnings -W clippy::pedantic

check-deps:
    cargo build
    @cargo tree -e features | rg '(reqwest|alloy-transport-http|HttpConnector|hyper-tls|native-tls)' && exit 1 || true
    cargo deny check bans
    cargo fmt --check

check-static:
    @rg -n --pcre2 '(TcpStream::connect|lookup_host|to_socket_addrs|reqwest|HttpConnector|ProviderBuilder::on_http|fetch\()' src railgun-runtime tests | rg -v '^([^:]*):(\s*//|\s*\*)' | rg -v '__undercover_deno_fetch\("https://example\.com"\)' && exit 1 || true
    cargo clippy --all-targets -- -D warnings -W clippy::pedantic

ping rpc="https://ethereum-sepolia-rpc.publicnode.com":
    cargo run --release -- ping --rpc {{rpc}}

signer-address:
    cargo run --release -- signer-address

runtime-bundle:
    cd railgun-runtime && npm run bundle:embedded

runtime-smoke:
    cd railgun-runtime && npm run bundle:embedded
    cargo run -- runtime-smoke

load-wallet mnemonic="test test test test test test test test test test test junk":
    cd railgun-runtime && npm run bundle:embedded
    cargo run -- load-wallet --railgun-mnemonic "{{mnemonic}}"

embedded-check:
    cargo fmt --check
    cd railgun-runtime && npm run bundle:embedded
    cargo run -- runtime-smoke
    cargo clippy --all-targets -- -D warnings -W clippy::pedantic
