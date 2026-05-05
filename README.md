# Undercover

Undercover is a proof of concept for Railgun transactions whose network
egress is owned by Rust and routed through Arti. The current active path
keeps everything in one process: Rust embeds Deno for the Railgun SDK, denies
network access inside JavaScript, and services SDK JSON-RPC/HTTP requests
through Arti.

The Docker/Node sidecar is still present as a fallback and comparison harness,
but it is no longer the preferred runtime boundary.

## Architecture

```
Rust CLI
  ├─ Tor client via Arti
  ├─ Alloy provider over an Arti-backed hyper connector
  ├─ Alloy wallet signer and transaction broadcaster
  └─ Embedded Deno worker
       └─ bundled Railgun SDK runtime
```

The embedded worker loads `embedded/railgun_runtime.iife.js`, generated from
`sidecar/runtime.mjs`. JavaScript cannot open sockets or use ambient fetch.
When the Railgun SDK needs JSON-RPC or GraphQL, it emits a reverse request to
Rust; Rust performs the request through Arti.

Railgun quick-sync uses:

`https://rail-squid.squids.live/squid-railgun-eth-sepolia-v2/graphql`

through the same Arti reverse-HTTP path.

## Requirements

- Rust 1.91+
- Node/npm for bundling the Railgun runtime
- `just` for the documented recipes
- Docker only if you want to exercise the legacy sidecar path

Install JS dependencies:

```sh
cd sidecar
npm install
```

Generate the embedded Railgun bundle:

```sh
just embedded-bundle
```

The generated `embedded/` files are build outputs and are intentionally not
committed.

## Quick Checks

Embedded smoke:

```sh
just embedded-smoke
```

Full embedded check:

```sh
just embedded-check
```

Static egress check:

```sh
just check-static
```

The embedded smoke verifies:

- Railgun SDK loads under embedded Deno.
- Deno `fetch` is denied.
- `Deno.connect` is denied.
- `node:net` is denied.
- writes outside artifacts are denied.
- broad env reads are denied.
- artifact reads are allowed.

## Common Commands

Check the signer address:

```sh
cargo run -- signer-address --private-key "$UNDERCOVER_PRIVATE_KEY"
```

Or use a Ledger for the public gas-payer account:

```sh
cargo run -- signer-address --ledger
```

Ledger options are available anywhere a public signer is required:

- `--ledger`: connect to the Ledger Ethereum app.
- `--ledger-index <n>`: use Ledger Live account index `n`; default is `0`.
- `--ledger-path <path>`: use a custom derivation path.
- `--chain-id <id>`: signer chain ID; default is Sepolia `11155111`.

The Ledger only protects the public EOA used to pay gas and broadcast
transactions. The Railgun wallet mnemonic is still loaded by the SDK runtime.

Ping an RPC endpoint through Arti:

```sh
cargo run -- ping --rpc https://ethereum-sepolia-rpc.publicnode.com
```

Load a Railgun wallet in the embedded runtime:

```sh
cargo run --features deno-runtime -- \
  load-wallet-smoke \
  --embedded \
  --railgun-mnemonic "$UNDERCOVER_RAILGUN_MNEMONIC"
```

Populate a Sepolia base-token shield transaction without broadcasting:

```sh
cargo run --features deno-runtime -- \
  shield-base-token \
  --embedded \
  --dry-run \
  --rpc https://ethereum-sepolia-rpc.publicnode.com \
  --amount-wei 1 \
  --ledger \
  --railgun-mnemonic "$UNDERCOVER_RAILGUN_MNEMONIC"
```

Refresh private balance through Arti:

```sh
cargo run --features deno-runtime -- \
  refresh-balance \
  --embedded \
  --rpc https://ethereum-sepolia-rpc.publicnode.com \
  --creation-block <wallet-creation-block> \
  --railgun-mnemonic "$UNDERCOVER_RAILGUN_MNEMONIC"
```

Populate an unshield transaction without broadcasting:

```sh
cargo run --features deno-runtime -- \
  unshield-base-token \
  --embedded \
  --dry-run \
  --rpc https://ethereum-sepolia-rpc.publicnode.com \
  --creation-block <wallet-creation-block> \
  --amount-wei 1 \
  --ledger \
  --railgun-mnemonic "$UNDERCOVER_RAILGUN_MNEMONIC"
```

## Legacy Sidecar

The Docker sidecar recipes remain available:

```sh
just check-sidecar
just sidecar-smoke
```

Use these only to compare behavior against the older boundary. New work should
prefer the embedded path unless the goal explicitly changes.

## Repository Map

- `src/embedded.rs`: embedded Deno worker and reverse-RPC pump.
- `src/transport.rs`: Arti-backed hyper connector.
- `src/rpc.rs`: Alloy provider construction over Arti.
- `src/main.rs`: CLI orchestration.
- `sidecar/runtime.mjs`: shared Railgun runtime logic.
- `sidecar/main.mjs`: Docker sidecar stdio adapter.
- `sidecar/build-embedded.mjs`: bundle generation for embedded Deno.
- `spec.md`: fuller design notes and historical plan.
- `AGENTS.md`: implementation guidance and verification notes.
- `wayfinding/`: exploratory artifacts retained for design history.

## Notes

Arti does not currently provide official Node, Deno, or Bun FFI bindings.
Those runtimes could call a custom native shim, but this project avoids that
extra ABI surface by keeping Arti in Rust and embedding JavaScript instead.
