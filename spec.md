# Undercover Spec

Undercover is a Rust CLI for Railgun transactions whose network egress is
owned by Rust and routed through Tor. Railgun SDK code runs inside an embedded
Deno worker in the same process. JavaScript has no network permission; when it
needs JSON-RPC or GraphQL data, it asks Rust to perform the request through
the Tor-backed transport.

## Current Shape

The active architecture is:

```
Rust CLI
  ├─ Tor client via Arti
  ├─ Alloy provider over a Tor-backed hyper connector
  ├─ Alloy wallet signer and transaction broadcaster
  └─ Embedded Deno worker
       └─ bundled Railgun SDK runtime
```

There is no Docker or Node sidecar in the product path. The JavaScript runtime
lives under `railgun-runtime/` only because the Railgun SDK is JavaScript. The
bundle generated from `railgun-runtime/runtime.mjs` is loaded by
`src/embedded.rs`.

## Security Invariants

- All external TCP egress from the Rust process goes through
  `TorClient::connect` via `src/transport.rs`.
- RPC DNS resolution is delegated to Tor by passing hostnames into
  `TorClient::connect`; the code must not pre-resolve RPC hosts.
- Embedded Deno must deny `fetch`, `Deno.connect`, `node:net`, writes outside
  `artifacts/`, and broad environment reads.
- The public EOA signer stays in Rust/Alloy. The Railgun mnemonic is passed
  only to the embedded SDK runtime.
- The JS runtime can request reverse JSON-RPC/HTTP, but Rust owns execution of
  those requests and routes them through Tor.

## Runtime Boundary

`src/railgun.rs` is the typed Rust facade over the embedded runtime:

- `health`
- `permission_smoke`
- `load_wallet`
- `populate_shield_base_token`
- `refresh_balance`
- `prepare_unshield_base_token`

`src/embedded.rs` owns Deno worker setup, permissions, bundle loading, and the
reverse-RPC pump. `railgun-runtime/runtime.mjs` owns Railgun SDK calls and is
bundled by `railgun-runtime/build-embedded.mjs`.

## CLI

The CLI commands are:

- `ping`: verify RPC reachability through Tor.
- `runtime-smoke`: verify embedded SDK load and denied JS egress.
- `load-wallet`: load a Railgun wallet and print its shielded address.
- `signer-address`: print the public gas-payer address.
- `shield`: populate and optionally broadcast a base-token shield transaction.
- `balance`: refresh and print the private base-token balance.
- `unshield`: prove, prepare, and optionally broadcast a base-token unshield
  transaction.

The default RPC is Sepolia publicnode. The default signer chain ID is Sepolia
`11155111`.

## Signing

Use Alloy's type system directly:

- Private keys parse into `PrivateKeySigner`, then `EthereumWallet`.
- Ledger accounts use `alloy-signer-ledger::LedgerSigner`, then
  `EthereumWallet`.
- Provider construction accepts `NetworkWallet<Ethereum>`; do not add a
  project-local signer enum.

The Ledger only protects the public EOA used to pay gas and broadcast
transactions. It does not replace the Railgun mnemonic consumed by the SDK.

## Verification

Required local checks:

```sh
cargo fmt --check
cargo check
cargo clippy --all-targets -- -D warnings -W clippy::pedantic
just runtime-smoke
just check-static
```

For stricter cleanup work, run:

```sh
cargo clippy --all-targets -- -D warnings -W clippy::pedantic
```

Generated files under `embedded/` and downloaded proving artifacts under
`artifacts/` are not committed.
