# Undercover

Undercover is a proof of concept for Railgun transactions whose network
egress is owned by Rust and routed through Tor. Rust embeds Deno for the
Railgun SDK, denies network access inside JavaScript, and services SDK
JSON-RPC/HTTP requests through Tor.

## Architecture

```
Rust CLI
  ├─ Tor client via Arti
  ├─ Alloy provider over a Tor-backed hyper connector
  ├─ Alloy wallet signer and transaction broadcaster
  └─ Embedded Deno worker
       └─ bundled Railgun SDK runtime
```

The embedded worker loads `embedded/railgun_runtime.iife.js`, generated from
`railgun-runtime/runtime.mjs`. JavaScript cannot open sockets or use ambient
fetch. When the Railgun SDK needs JSON-RPC or GraphQL, it emits a reverse
request to Rust; Rust performs the request through Tor.

Railgun quick-sync uses
`https://rail-squid.squids.live/squid-railgun-eth-sepolia-v2/graphql`
through the same Tor reverse-HTTP path.

## Requirements

- Rust 1.91+
- Node/npm for bundling the Railgun runtime
- `just` for the documented recipes

Install JS dependencies:

```sh
cd railgun-runtime
npm install
```

Generate the embedded Railgun bundle:

```sh
just runtime-bundle
```

The generated `embedded/` files are build outputs and are intentionally not
committed.

## Checks

```sh
just runtime-smoke
just embedded-check
just check-static
```

The runtime smoke verifies that the Railgun SDK loads under embedded Deno and
that Deno `fetch`, `Deno.connect`, `node:net`, writes outside artifacts, and
broad env reads are denied while artifact reads are allowed.

## Commands

Check the public signer address:

```sh
cargo run -- signer-address --private-key "$UNDERCOVER_PRIVATE_KEY"
cargo run -- signer-address --ledger
```

Ledger options are available anywhere a public signer is required:

- `--ledger`: connect to the Ledger Ethereum app.
- `--ledger-index <n>`: use Ledger Live account index `n`; default is `0`.
- `--ledger-path <path>`: use a custom derivation path.
- `--chain-id <id>`: signer chain ID; default is Sepolia `11155111`.

The Ledger only protects the public EOA used to pay gas and broadcast
transactions. The Railgun wallet mnemonic is still loaded by the SDK runtime.

Ping an RPC endpoint through Tor:

```sh
cargo run -- ping --rpc https://ethereum-sepolia-rpc.publicnode.com
```

Load a Railgun wallet:

```sh
cargo run -- load-wallet \
  --railgun-mnemonic "$UNDERCOVER_RAILGUN_MNEMONIC"
```

Populate a Sepolia base-token shield transaction without broadcasting:

```sh
cargo run -- shield \
  --dry-run \
  --amount-wei 1 \
  --ledger \
  --railgun-mnemonic "$UNDERCOVER_RAILGUN_MNEMONIC"
```

Refresh private balance through Tor:

```sh
cargo run -- balance \
  --creation-block <wallet-creation-block> \
  --railgun-mnemonic "$UNDERCOVER_RAILGUN_MNEMONIC"
```

Populate an unshield transaction without broadcasting:

```sh
cargo run -- unshield \
  --dry-run \
  --creation-block <wallet-creation-block> \
  --amount-wei 1 \
  --ledger \
  --railgun-mnemonic "$UNDERCOVER_RAILGUN_MNEMONIC"
```

## Repository Map

- `src/main.rs`: process setup and CLI dispatch.
- `src/cli.rs`: clap command and argument definitions.
- `src/commands.rs`: command handlers.
- `src/railgun.rs`: typed Railgun runtime API over embedded Deno.
- `src/embedded.rs`: embedded Deno worker and reverse-RPC pump.
- `src/transport.rs`: Tor-backed hyper connector.
- `src/rpc.rs`: Alloy provider construction over Arti.
- `src/signer.rs`: Alloy local-key and Ledger signer construction.
- `railgun-runtime/runtime.mjs`: shared Railgun SDK runtime logic.
- `railgun-runtime/build-embedded.mjs`: bundle generation for embedded Deno.
- `spec.md`: fuller design notes and historical plan.
- `AGENTS.md`: implementation guidance and verification notes.
- `wayfinding/`: exploratory artifacts retained for design history.

## Notes

Arti, the Rust Tor library, does not currently provide official Node, Deno, or
Bun FFI bindings. Those runtimes could call a custom native shim, but this
project avoids that extra ABI surface by keeping Tor in Rust and embedding
JavaScript instead.
