# Hermetic Agent Notes

## Embedded runtime direction

- The active single-process path is Rust owning Tor via Arti and embedding Deno.
  JavaScript must not own network egress. The embedded worker should keep
  Deno network permissions denied and ask Rust to service JSON-RPC/HTTP
  through Tor.
- Treat `railgun-runtime/runtime.mjs` as the shared Railgun runtime surface.
  `src/embedded.rs` is the embedded Deno adapter.
- Arti is a Rust crate integration point, not a ready-made JS FFI binding.
  Node, Deno, or Bun could call a custom C/N-API shim, but that would be a
  new native binding surface maintained here. Prefer Rust-owned Tor via Arti plus
  embedded JS host ops for this project.
- Running Tor via a SOCKS/HTTP CONNECT proxy is useful for comparison, but it
  reintroduces a local proxy process/socket boundary. Do not switch back to
  that unless the explicit goal changes.

## Ethereum signing

- Use Alloy's wallet types directly. Public signer sources should convert
  into `EthereumWallet`, and provider construction should accept
  `NetworkWallet<Ethereum>` rather than a project-local signer enum.
- Ledger support is provided through `alloy-signer-ledger::LedgerSigner`.
  Prefer `HDPath::LedgerLive(index)` for the normal account-index CLI and
  `HDPath::Other(path)` only when the user supplies an explicit derivation
  path.
- Keep the public EOA signer boundary separate from Railgun wallet loading.
  The public signer pays gas and broadcasts transactions; the Railgun mnemonic
  is still owned by the SDK runtime.

## Verification

- Production verification should be CLI-level, not example-level:
  `doctor`, `wallet import`, `wallet list`, `shield --dry-run`, `balance`,
  and unshield preflight.
- `refresh_balance` and unshield preflight should complete via Railgun
  quick-sync GraphQL over Tor plus local balance decryption. Avoid the SDK
  slow `eth_getLogs` scan from deployment block; it can time out against
  public RPC endpoints.
- Keep the embedded permission smoke meaningful: Deno `fetch`,
  `Deno.connect`, `node:net`, writes outside artifacts, and broad env reads
  should be denied, while artifact reads should be allowed.
- Generated bundles under `embedded/` are build outputs from
  `railgun-runtime/build-embedded.mjs`; do not commit them.

## Wayfinding

- `wayfinding/deno-embedding/` contains exploratory Deno embedding proofs.
  They are retained for design history only and should not be part of the
  normal build or verification path.
- `wayfinding/tor-transport-spike/` contains the old Tor transport spike. It is
  useful design history, but the CLI now owns the supported Tor verification
  path.
