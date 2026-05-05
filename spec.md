# Undercover

Proof of concept for sending anonymous Ethereum transactions by combining
Arti (embedded Tor in Rust) with Railgun (zk-SNARK on-chain privacy on
Ethereum). The defining property of the system: every byte of network
egress between this process and any Ethereum infrastructure travels
through a Tor circuit, while every on-chain transaction in the demo is
a Railgun shielded operation.

## Goal

Demonstrate, end to end on the selected Railgun testnet (Sepolia if
supported), a flow that:

1. Shields native testnet ETH from a clearnet EOA into a Railgun
   shielded balance.
2. Performs a private transfer between two shielded addresses.
3. Unshields to a fresh EOA.

All RPC and chain-scan traffic in (1)-(3) is routed through Arti.
Railgun proof generation runs in a Node sidecar container launched
with Docker `--network none`, `--read-only`, `--cap-drop ALL`, and
`--security-opt no-new-privileges`. The Rust process communicates with
the container over stdin/stdout JSON-RPC. This is intentionally a
process/container boundary for the PoC; embedding Deno or another JS
runtime can be tried later, but is not the current implementation path.

The end-to-end target chain is the chain on which Railgun's contracts
are currently deployed for testing. Sepolia is the working assumption,
but the Railgun deployments registry is the source of truth and must
be checked before M2 (see Q5). If Sepolia is not actively supported by
the SDK constants and deployed contracts at implementation time, the
PoC retargets to whichever testnet Railgun publishes addresses for,
without changes to the architecture.

## Non-goals

- Production wallet UX, persistent encrypted seed storage, key recovery.
- Mainnet deployment.
- Custom relayer or broadcaster.
- ERC-20 allowance setup. ERC-20 support is a follow-up because an
  `approve` transaction is not itself a Railgun shielded operation and
  would weaken the PoC's "all demo transactions are Railgun
  operations" property.
- Multi-chain (Polygon, BNB, Arbitrum).
- Mobile or browser targets.
- Reimplementing any part of Railgun's circuits or SNARK pipeline.

## Threat model

Adversary capabilities assumed:

- Passive: the adversary observes RPC endpoint logs, including request
  source IPs, request timing, and request content.
- Active: the adversary may run RPC endpoints, Railgun broadcasters,
  block explorers, and chain analytics services that the user touches.
- The adversary cannot break Tor's anonymity properties at the network
  layer.
- The adversary cannot break Groth16 or the Railgun circuit
  construction.
- The adversary does not have local code execution on the user's
  machine.

What the system hides:

- IP-to-shielded-address linkability for shield, transfer, and unshield
  operations (provided by Arti).
- Shielded sender, shielded recipient, and amount linkability inside
  private transfer calldata (provided by Railgun).

What the system does not hide:

- Timing and amount correlation between shield and unshield events.
  This is a known property of Railgun, not addressed here.
- Gas-payer linkability in self-broadcast mode. For the PoC, the same
  EOA may pay gas for shield, transfer, and unshield transactions; an
  on-chain observer can link those transactions by gas payer even
  though the shielded payload remains private. A public broadcaster or
  relayer changes this property and is tracked as a follow-up.
- Local-machine compromise.
- DNS leaks from misconfigured runtime code. Mitigated by passing
  hostnames (not pre-resolved IPs) into `TorClient::connect`, which
  resolves names inside the circuit, plus running the sidecar in a
  Docker container with `--network none`.

## Architecture

```
+----------------------------+      stdin (JSON-RPC)    +---------------------------+
|  Rust binary               |  --------------------->  |  Node sidecar container   |
|  `undercover`              |  <---------------------  |  Docker --network none    |
|                            |    stdout (JSON-RPC)     |  --allow-read=ALLOW       |
|  - arti_client TorClient   |    stderr (logs)         |                           |
|  - alloy Provider over     |                          |  npm:@railgun-community/  |
|    custom hyper connector  |                          |    wallet  (engine)       |
|  - sidecar driver          |                          |                           |
|  - EOA signer (LocalSigner)|                          |  in-memory:               |
|  - flow orchestrator       |                          |   - Railgun wallet keys   |
|  - Railgun event sync      |                          |   - note DB               |
|  - Merkle-root reader      |                          |   - merkle tree state     |
+-------------+--------------+                          +---------------------------+
              |
              | TCP via Arti circuit (TorClient::connect)
              v
+--------------------------------------------------+
| Selected testnet JSON-RPC endpoint (pending Q5)  |
| serving Ethereum chain + Railgun contract reads  |
+--------------------------------------------------+
```

System boundary invariants:

- Every TCP connection originating from this system enters the network
  through `arti_client::TorClient::connect` inside the Rust process.
- The sidecar has no container network interface (`docker run
  --network none`). Attempts to open sockets or call `fetch` fail at
  runtime.
- The sidecar therefore cannot do its own scanning; Rust feeds it
  Railgun event logs over stdio (see § Data flow).
- This is not a complete OS sandbox. A compromised Node runtime or kernel-level
  bypass is out of scope for the PoC; both an OS sandbox and a
  network namespace are tracked as hardening follow-ups.

## Key custody and state ownership

Three secrets exist in the system. The EOA key and Tor identity have a
single long-lived owner. The Railgun mnemonic transits Rust once at
startup because the PoC accepts it as CLI/env input, but the derived
Railgun wallet keys and note state live only in the sidecar.

1. **EOA private key** (used to sign on-chain transactions and pay
   gas). Lives in the Rust process only. Loaded from `--signer-hex`
   in the original design; the current CLI uses `--private-key`
   (or `UNDERCOVER_PRIVATE_KEY`) into
   `alloy::signers::local::PrivateKeySigner`. Never written to disk.
   Never serialized over stdio. The sidecar receives only the EOA's
   public address.

2. **Railgun wallet (mnemonic and derived spending + viewing keys)**.
   The mnemonic is supplied to Rust via `--railgun-mnemonic` (or the
   matching env var), serialized exactly once into the `load_wallet`
   JSON-RPC request, then dropped/zeroized by Rust after the sidecar
   acknowledges the call. Rust never derives, stores, or uses the
   Railgun spending/viewing keys. The derived wallet keys live in the
   Node sidecar container only, are held in memory for the lifetime of
   the sidecar process, and are never written to disk; the container
   runs read-only. After `load_wallet`, the Rust side keeps
   only the opaque `wallet_id` and derived 0zk address returned by the
   sidecar.

3. **Tor identity (guard descriptors, bootstrap state)**. Lives on
   disk under `./.arti/state` (Tor's persistent state). Owned by
   Arti, not application code. This is the conventional Tor model;
   do not rotate.

State that is needed for proof construction but is not secret:

- **Note DB**: the wallet's view of its own shielded notes, derived
  by scanning Railgun event logs and decrypting commitments with the
  viewing key. Lives in the sidecar's memory. Populated by the Rust
  side feeding event log batches over stdio (`ingest_events`),
  because the sidecar has no network. Lost when the sidecar exits;
  re-derived on next startup by replaying logs.

- **Merkle tree state**: the commitment tree maintained by Railgun's
  contract on chain. The current root and the merkle paths required
  for spending proofs are derived from the same event stream as the
  note DB.

This split allows the system to make a sharp claim: even a fully
compromised sidecar binary can deanonymize the user's shielded
balance and forge proofs against the user's notes (because it holds
the spending key), but it cannot move the user's clearnet ETH (the
EOA key never leaves Rust) and it cannot exfiltrate by network
socket (the container has no network namespace).

Conversely, a compromised Rust process can move clearnet ETH and can
capture the Railgun mnemonic during startup forwarding, so this split
does not protect the Railgun wallet from a malicious orchestrator.
The useful boundary is narrower: after startup, normal Rust flow code
does not derive or retain Railgun wallet keys, and all proof-state
machinery remains inside the no-network sidecar.

This is not a defense-in-depth boundary worth much against a
sophisticated attacker (both processes run as the same OS user). It
is a structural boundary that keeps the proof of concept honest about
where each capability sits, and that pairs with Docker `--network none` to
guarantee the proof builder is an air-gapped function from the
network's point of view.

## Components

### Rust orchestrator

Crate name: `undercover`. Single binary.

Dependency pins (latest stable as of 2026-05-05, verified against
crates.io):

- `arti-client = { version = "0.41", default-features = false,
  features = ["tokio", "rustls"] }`. `onion-service-client` is
  intentionally not enabled: the PoC connects only to clearnet RPC
  endpoints over Tor circuits, so the onion-service surface is
  unnecessary and broadens the Arti API for no stated need.
- `tor-rtcompat = { version = "0.41", default-features = false,
  features = ["tokio", "rustls"] }`.
- **Alloy: do not depend on the `alloy` meta-crate.** The meta-crate's
  `providers` feature transitively enables `rpc-client`, which
  transitively enables `transport-http`, which pulls in
  `alloy-transport-http` and reqwest. Instead, depend on the
  individual workspace crates and pick exactly the transport story
  we want:
    - `alloy-rpc-client = { version = "2.0.4", default-features = false }`.
    - `alloy-network = { version = "2.0.4", default-features = false }`.
    - `alloy-provider = { version = "2.0.4", default-features = false }`
      (no `reqwest`, `hyper`, or other built-in transport feature).
    - `alloy-signer-local = { version = "2.0.4", default-features = false }`.
    - `alloy-rpc-types-eth = { version = "2.0.4", default-features = false }`.
    - `alloy-transport = { version = "2.0.4", default-features = false }`.
    - `alloy-eips = { version = "2.0.4", default-features = false }`,
      `alloy-consensus = { version = "2.0.4", default-features = false }`,
      and `alloy-primitives = { version = "1.4.1", default-features = false }`
      as required by the compile spike.
  Do NOT depend on `alloy-transport-http`. The custom transport is
  built directly: a hyper client with `ArtiConnector` as its connector,
  wrapped in a thin `tower::Service` adapter that satisfies
  `alloy-transport`'s service trait. The exact sub-crate set and the
  feature flags must be validated by the M1 compile spike (see
  Milestones), since alloy's workspace surface evolves and a feature
  graph that does not contain `alloy-transport-http` today may sprout
  one tomorrow.
- `hyper = { version = "1.9", features = ["client", "http1", "http2"] }`.
- `hyper-util = { version = "0.1", features = ["client", "tokio"] }`.
- `rustls = { version = "0.23", default-features = false,
  features = ["std", "tls12", "ring"] }`. The `ring` feature selects
  ring as the crypto provider; without it, rustls 0.23 has no
  installed provider and TLS handshakes fail at runtime. The Rust
  binary calls
  `rustls::crypto::ring::default_provider().install_default()` at
  the top of `main` before any TLS context is constructed. (Choosing
  `aws_lc_rs` instead is acceptable and gains FIPS posture but adds a
  C build dependency; the PoC defaults to `ring` for portability.)
- `tokio-rustls = "0.26"`.
- `webpki-roots = "0.26"`.
- `tokio = { version = "1.52", features = ["full"] }`.
- `serde = { version = "1", features = ["derive"] }`,
  `serde_json = "1"`, `anyhow = "1"`, `thiserror = "2"`.
- `zeroize = "1"` for clearing the forwarded Railgun mnemonic buffer
  after `load_wallet` succeeds or fails.
- `tracing = "0.1"`, `tracing-subscriber = "0.3"`.
- `clap = { version = "4.6", features = ["derive", "env"] }`.
- `url = "2"`, `bytes = "1"`, `tower = "0.5"`.
- `http = "1"`, `http-body-util = "0.1"`.

Banned crates and methods (enforced by `clippy.toml` `disallowed_methods`
and a `cargo deny` rule for crate ban list):

- Crate `reqwest`: not a direct dep, and not allowed as a transitive
  dep on the binary's dependency graph (`cargo deny ban reqwest`).
  Rationale: reqwest pools its own connector and would silently
  bypass `ArtiConnector` if any code reached for it.
- Method `tokio::net::TcpStream::connect`: disallowed everywhere.
- Method `std::net::TcpStream::connect`: disallowed everywhere.
- Method `std::net::ToSocketAddrs::to_socket_addrs`: disallowed
  everywhere (would force OS-level DNS resolution, leaking outside
  Tor).
- Method `tokio::net::lookup_host`: disallowed everywhere.
- Method `hyper_util::client::legacy::connect::HttpConnector::new` and
  any `HttpConnector` constructor: disallowed everywhere; the only
  permitted hyper connector is `ArtiConnector` from `transport.rs`.

Patch versions resolve via Cargo.lock to the latest matching the
ranges above (verified 2026-05-05: arti-client 0.41.0, alloy 2.0.4,
hyper 1.9.0, hyper-util 0.1.20, rustls 0.23.40, tokio 1.52.2, clap
4.6.1).

Module layout:

- `src/main.rs`: clap CLI, entry point.
- `src/arti.rs`: Arti bootstrap.
- `src/transport.rs`: hyper connector backed by Arti.
- `src/rpc.rs`: alloy provider construction over the custom transport.
- `src/sidecar.rs`: Docker child process management and JSON-RPC over
  stdio.
- `src/flow.rs`: shield, transfer, unshield orchestration.
- `src/error.rs`: typed errors.

Public signatures, inlined here so that this spec is self-contained:

```rust
// src/arti.rs
pub async fn bootstrap(state_dir: &Path, cache_dir: &Path)
    -> anyhow::Result<arti_client::TorClient<tor_rtcompat::PreferredRuntime>>;

pub fn isolated_for(
    client: &arti_client::TorClient<tor_rtcompat::PreferredRuntime>,
    label: IsolationLabel,
) -> arti_client::TorClient<tor_rtcompat::PreferredRuntime>;

pub enum IsolationLabel {
    EventSync,    // log/state polling phase
    Shield,       // shield submit + receipt
    Transfer,     // private transfer submit + receipt
    Unshield,     // unshield submit + receipt
}
```

`bootstrap` uses persistent Tor state and cache directories
(`./.arti/state` and `./.arti/cache` by default; configurable). Tor
guard relays are sticky for a reason: rotating guard state every run
makes the user pick fresh guards constantly, which over time
increases the probability of selecting a hostile guard. Persistent
state preserves Tor's built-in guard discipline.

Per-phase unlinkability comes from `isolated_for`, which calls
`TorClient::isolated_client` (Arti's `IsolationToken`-backed
mechanism) so each phase's TCP streams are pinned to a fresh circuit
that cannot share a circuit with streams from any other phase. The
event-sync phase is isolated separately from each tx phase to avoid
linking "user X sees these notes" with "user X submits this tx" via
shared exit relay.

Returns once Arti reports the directory bootstrap is ready.

```rust
// src/transport.rs
pub struct ArtiConnector {
    tor: arti_client::TorClient<tor_rtcompat::PreferredRuntime>,
}

impl tower::Service<http::Uri> for ArtiConnector {
    type Response = hyper_util::rt::TokioIo<arti_client::DataStream>;
    type Error = anyhow::Error;
    type Future = std::pin::Pin<Box<
        dyn std::future::Future<Output = Result<Self::Response, Self::Error>>
        + Send
    >>;
    fn poll_ready(&mut self, _: &mut std::task::Context<'_>)
        -> std::task::Poll<Result<(), Self::Error>>;
    fn call(&mut self, req: http::Uri) -> Self::Future;
}
```

Inside `call`: parse host and port from the URI (default 443 for https,
80 for http), construct a `TorAddr::from((host, port))`, await
`self.tor.connect(addr)`, wrap the resulting `DataStream` in `TokioIo`.
Return the wrapped stream. TLS is negotiated by hyper above this
connector using rustls; the connector itself only provides the TCP
substrate.

```rust
// src/rpc.rs
pub fn provider(
    tor: arti_client::TorClient<tor_rtcompat::PreferredRuntime>,
    rpc_url: url::Url,
) -> impl alloy::providers::Provider;
```

`tor` is expected to be an `isolated_client` returned by
`arti::isolated_for(...)` so that all requests issued by this provider
ride a circuit pinned to one phase. The caller constructs one provider
per phase and passes a freshly isolated client each time.

The provider builds an alloy JSON-RPC transport on top of a hyper
client whose connector is `ArtiConnector`. The exact generic
instantiation depends on alloy 2.x's transport surface; the contract
is that every HTTP request issued by this provider passes through
`ArtiConnector::call`. No reqwest. No `HttpConnector`. No direct TCP.

```rust
// src/sidecar.rs
pub struct Sidecar {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::io::BufReader<tokio::process::ChildStdout>,
    next_id: u64,
}

impl Sidecar {
    pub async fn spawn(workdir: &std::path::Path) -> anyhow::Result<Self>;
    pub async fn call<Req: serde::Serialize, Res: serde::de::DeserializeOwned>(
        &mut self,
        method: &str,
        params: Req,
    ) -> anyhow::Result<Res>;
    pub async fn shutdown(self) -> anyhow::Result<()>;
}
```

`spawn` invokes Docker with the exact argv shown in the Sidecar section
below. `call` writes a single-line JSON-RPC request to stdin, then
reads exactly one line from stdout and decodes it. `shutdown` closes
stdin and awaits child exit.

CLI subcommands (`src/main.rs`):

- `undercover ping --rpc <URL>`: bootstraps Arti, builds provider, calls
  `eth_blockNumber` and `eth_chainId`. Prints both. Verifies that the
  network path works.
- `undercover sidecar-smoke`: spawns the sidecar, calls the `health`
  method, prints the response, shuts down.
- `undercover load-wallet-smoke --railgun-mnemonic <BIP39>`:
  initializes the Railgun wallet in the sidecar and returns the
  wallet ID plus 0zk address.
- `undercover shield-base-token --rpc <URL> --private-key <HEX>
  --railgun-mnemonic <BIP39> --amount-wei <wei>`: loads the Railgun
  wallet, asks the sidecar to populate Sepolia V2 RelayAdapt base-token
  shield calldata, signs in Rust, and broadcasts through Arti. Add
  `--dry-run` to stop after calldata construction.
- `undercover signer-address --private-key <HEX>`: prints the EOA
  address to fund before running `shield-base-token`.

### Node sidecar container

Layout:

- `sidecar/main.mjs`: JSON-RPC dispatcher over stdio.
- `sidecar/package.json`, `sidecar/package-lock.json`: pinned npm
  packages.
- `sidecar/Dockerfile`: container image for the sidecar runtime.
- `sidecar/scripts/fetch_artifacts.ts`: one-shot helper to download
  Railgun proving artifacts. Run separately, not during the main flow.
- `artifacts/`: created by `fetch_artifacts.ts`, holds proving keys.

Imports (`sidecar/package.json`):

```json
{
  "dependencies": {
    "@railgun-community/wallet": "10.8.6",
    "@railgun-community/shared-models": "8.0.1"
  }
}
```

Versions verified against npm on 2026-05-05.

Spawn argv from Rust:

```
docker run --rm -i \
    --network none \
    --read-only \
    --cap-drop ALL \
    --security-opt no-new-privileges \
    --mount type=bind,source=$PWD/artifacts,target=/app/artifacts,readonly \
    undercover-sidecar:dev
```

Container restrictions:

- `--network none`: no external or host network path.
- `--read-only`: container filesystem is not writable.
- `--cap-drop ALL` and `no-new-privileges`: no ambient capability
  escalation.
- Only `./artifacts` is mounted into the container, read-only.

Stdio wire format: line-delimited JSON. Exactly one request per line on
stdin produces exactly one response per line on stdout. Logs go to
stderr.

JSON-RPC method contracts (defined inline so this spec is executable
without other documents). All methods use `"jsonrpc":"2.0"`. Examples
below abbreviate it.

`health`. Confirms the sidecar started and the SDK loaded under Node.

```jsonc
// request
{"id":1,"method":"health"}
// response
{"id":1,"result":{
   "sdk_version":"10.8.6",
   "shared_models_version":"8.0.1",
   "node_compat":true
}}
```

`load_artifacts`. Loads Groth16 proving keys from disk into the SDK's
in-memory artifact store. Reads only from the path supplied, which
must be inside the `--allow-read` allow-list.

```jsonc
// request
{"id":2,"method":"load_artifacts","params":{"path":"./artifacts"}}
// response
{"id":2,"result":{"loaded":true,"circuits":["transfer","unshield"]}}
```

`load_wallet`. Initializes the Railgun wallet inside the sidecar from
a mnemonic. Returns the derived 0zk address. The mnemonic is held in
memory only; the sidecar has no `--allow-write` and cannot persist it.

```jsonc
// request
{"id":3,"method":"load_wallet","params":{
   "mnemonic":"<bip-39 phrase>",
   "encryption_key":"<32-byte hex, used by SDK for in-memory note
                     encryption only>",
   "chain":{"type":"EVM","id":11155111}
}}
// response
{"id":3,"result":{
   "wallet_id":"<sdk wallet handle>",
   "shielded_address":"0zk1q..."
}}
```

The mnemonic source for the PoC is a CLI flag bound to an env var
(`UNDERCOVER_RAILGUN_MNEMONIC`). Rust forwards it exactly once to
`load_wallet`, then zeroizes/drops its local buffer after the sidecar
acknowledges the call. For the demo, the user is expected to use a
freshly generated test mnemonic, not a production wallet seed.
Persistent encrypted storage is out of scope.

`ingest_events`. Feeds a batch of Railgun-contract event logs into
the wallet's note scanner. The Rust side fetches these logs through
Arti and ships them to the sidecar in chronological order. The
sidecar updates its note DB and merkle state and returns the highest
block number it has now scanned to.

```jsonc
// request
{"id":4,"method":"ingest_events","params":{
   "wallet_id":"<sdk wallet handle>",
   "from_block":"0x...",
   "to_block":"0x...",
   "logs":[
     {"address":"0xRailgunV3...","topics":["0x..."],"data":"0x...",
      "blockNumber":"0x...","transactionHash":"0x...","logIndex":"0x...",
      "removed":false},
     // more logs in chronological order
   ]
}}
// response
{"id":4,"result":{
   "scanned_to_block":"0x...",
   "notes":42,
   "spent_notes":3,
   "merkle_root":"0x..."
}}
```

`merkle_root`. Returns the current local merkle root (after all
ingested events). Used by Rust to compare with the on-chain root
fetched via `eth_call` to verify scan completeness before each tx.

```jsonc
// request
{"id":5,"method":"merkle_root","params":{
   "wallet_id":"<sdk wallet handle>"
}}
// response
{"id":5,"result":{"merkle_root":"0x..."}}
```

`build_shield_tx`. Builds calldata for the on-chain shield call. Does
not require the wallet's spending key (shielding is a public
encryption to a 0zk address). Provided for symmetry with the other
build_* methods.

```jsonc
// request
{"id":6,"method":"build_shield_tx","params":{
   "from":"0x9aB3...",
   "shielded_recipient":"0zk1qy...",
   "token":"native",
   "amount":"1000000000000000000",
   "chain":{"type":"EVM","id":11155111}
}}
// response
{"id":6,"result":{"to":"0xRailgunV3...","data":"0x...","value":"0x0"}}
```

`build_private_transfer`. Constructs a Groth16-backed shielded
transfer using the in-memory wallet (spending key, scanned notes,
merkle paths). The sidecar selects notes, builds witness, generates
proof, and returns calldata.

```jsonc
// request
{"id":7,"method":"build_private_transfer","params":{
   "wallet_id":"<sdk wallet handle>",
   "shielded_to":"0zk1...",
   "token":"native",
   "amount":"1000000",
   "memo":null,
   "chain":{"type":"EVM","id":11155111}
}}
// response
{"id":7,"result":{
   "to":"0xRailgunV3...",
   "data":"0x...",
   "spent_note_commitments":["0x...","0x..."]
}}
```

`build_unshield_tx`. Same shape as private transfer, but the
destination is a clearnet EOA.

```jsonc
// request
{"id":8,"method":"build_unshield_tx","params":{
   "wallet_id":"<sdk wallet handle>",
   "to":"0xRecipient...",
   "token":"native",
   "amount":"500000",
   "chain":{"type":"EVM","id":11155111}
}}
// response
{"id":8,"result":{
   "to":"0xRailgunV3...",
   "data":"0x...",
   "spent_note_commitments":["0x..."]
}}
```

Error envelope on any failure:

```jsonc
{"id":N,"error":{"code":-32603,"message":"...","data":{...}}}
```

Custody and signing summary:

- The EOA private key never enters the sidecar. Signing of every
  on-chain transaction (shield, transfer, unshield, plus any helper
  reads that need a from-address) happens in Rust via
  `alloy::signers::local::PrivateKeySigner`.
- The Railgun mnemonic transits Rust once during startup forwarding,
  but Rust never derives or stores the Railgun spending/viewing keys.
  The sidecar produces calldata only.
- The sidecar's `params.chain.id` is set from the chosen testnet in
  `chains.toml`. Examples in this document use Sepolia's `11155111`
  until Q5 resolves; implementation must match the SDK's `NetworkName`
  constants and the deployed Railgun contract addresses on the chosen
  testnet.

## Data flow

The demo runs four phases. Each phase uses a distinct Arti circuit
obtained via `arti::isolated_for(...)` so the streams cannot share an
exit relay across phases.

Phase 0, event sync (circuit: `EventSync`).

The Rust orchestrator obtains the Railgun deployment block (the block
the contracts were deployed at) and the current chain head via
`eth_blockNumber` and `eth_call` against the chosen testnet provider. It
issues `eth_getLogs` requests in chunks (e.g. 5,000 blocks) over the
Railgun contracts' event topics, paginating from the deployment block
or from the last-scanned block on disk if a checkpoint exists. As log
batches arrive, Rust calls the sidecar's `ingest_events` with each
batch in chronological order. The sidecar updates its note DB and
merkle tree. After the last batch, Rust calls `eth_call` against the
Railgun contract to read the current on-chain merkle root, and calls
the sidecar's `merkle_root` to compare. If they match, the wallet is
synced; if not, the orchestrator fetches further blocks until they
agree.

Phases 1-3 (circuit: `Shield`, `Transfer`, `Unshield` respectively).

For each phase, the orchestrator:

1. Constructs a fresh provider over an isolated client for that
   phase's `IsolationLabel`.
2. Calls the relevant sidecar `build_*` method. The sidecar returns
   calldata (and, for transfer/unshield, the list of spent note
   commitments so Rust can later verify they appeared on-chain).
3. Fetches the nonce, gas price, and gas limit via the
   phase-isolated provider.
4. Signs the transaction with the EOA signer (Rust-only).
5. Submits the signed transaction via the phase-isolated provider.
6. Polls for the receipt via the phase-isolated provider.
7. After the receipt confirms, runs a small incremental sync: Rust
   fetches logs from `(last_scanned_block + 1)` to the receipt's
   block, and calls `ingest_events` again so the sidecar's state
   reflects the new note set before the next phase starts.

The split is deliberate. Only proof construction needs the Railgun
SDK and Railgun's spending authority; the SDK lives in the sidecar
and is fed log batches over stdio. Submission, gas estimation, nonce
fetching, log fetching, and receipt polling all use the same
Arti-tunneled alloy provider in Rust. This makes the egress audit
trivial: every external connection is a `TorClient::connect` call
inside `transport.rs`.

## Security invariants

I1. **All TCP egress goes through Arti.** Every TCP connection
originating from this system is opened by `TorClient::connect`
inside `ArtiConnector::call` in `src/transport.rs`. There is no
other call site that opens a socket. Enforced by `clippy.toml`
`disallowed_methods` covering `tokio::net::TcpStream::connect`,
`std::net::TcpStream::connect`, `tokio::net::lookup_host`,
`std::net::ToSocketAddrs::to_socket_addrs`, and any
`hyper_util::client::legacy::connect::HttpConnector` constructor;
plus `cargo deny` banning the `reqwest` crate and any crate that
brings its own connector in the dependency graph.

I2. **Sidecar has no network capability inside the container.**
Enforced by launching the Node sidecar with Docker `--network none`,
`--read-only`, `--cap-drop ALL`, and `--security-opt no-new-privileges`.
The sidecar permission smoke asserts `fetch`, public socket connects,
and `node:net` loopback connects are denied while read-only artifact
access still works. This is the current PoC boundary; deeper OS
sandboxing is a follow-up.

I3. **Key custody is split, not pooled.** The EOA private key lives
only in Rust and is loaded from `--private-key` or
`UNDERCOVER_PRIVATE_KEY`. The Railgun mnemonic
transits Rust once in the `load_wallet` request, then Rust
zeroizes/drops its local buffer. The derived Railgun spending/viewing
keys live only in the sidecar. The EOA key is never serialized over
stdio; the Railgun mnemonic is serialized only in the one direction
that introduces it to the sidecar. Calldata returned by the sidecar is
always unsigned.

I4. **DNS resolution flows through Tor.** Hostnames (not pre-resolved
IPs) are passed to `TorClient::connect`, which resolves names inside
the Tor circuit. There is no `to_socket_addrs` call anywhere in the
egress path. DNS-leak invariant is verified by the acceptance check
that asserts no UDP/53 traffic during a run (see § Build, run, verify).

I5. **No fallback paths.** Any failure surfaces as an error to the
caller. The system never attempts a direct connection if Arti is
unavailable, and never silently degrades to clearnet. There are no
"if this fails, try X" branches in `transport.rs`.

I6. **Persistent Tor state with per-phase circuit isolation.** Tor
guards persist across runs under `./.arti/state` (Tor's
recommended discipline for guard rotation). Within a run, each phase
(`EventSync`, `Shield`, `Transfer`, `Unshield`) uses
`TorClient::isolated_client` so its streams cannot share a circuit
with another phase's streams. Per-stream isolation within a phase is
a follow-up (Q3 in the previous spec, now retired).

I7. **The sidecar's stdout is reserved for JSON-RPC responses only.**
All diagnostic output goes to stderr. The sidecar driver in Rust
treats a non-JSON line on stdout as a fatal error, since it implies
the sidecar's contract was violated (and therefore the air-gap claim
of Docker `--network none` cannot be reasoned about cleanly).

I8. **No reqwest, no `HttpConnector`, no transport that builds its
own connector pool.** The only permitted hyper connector in the
binary's dependency graph is `ArtiConnector`. Enforced as in I1.

I9. **No clearnet I/O during the demo flow except Arti's own guard
traffic.** Verified at runtime by the acceptance check that
captures all egress and asserts only Tor relay destinations
(see § Build, run, verify).

## Build, run, verify

Justfile (root of the project):

```
default:
    @just --list

build:
    cargo build --release
    cargo deny check bans
    cargo clippy --all-targets -- -D warnings

# === Verification loops (see § Verification loops) ===

# L1. Dependency / feature-graph hygiene.
check-deps:
    cargo build
    @echo "feature-graph banned-dep grep:"
    @cargo tree -e features | rg \
      '(reqwest|alloy-transport-http|HttpConnector|hyper-tls|native-tls)' \
      && exit 1 || true
    cargo deny check bans
    cargo fmt --check

# L2. Transport-spike call counter.
spike:
    cargo run --release --example spike
    cargo test --release --test spike_connector_counter

# L3. Static no-clearnet ripgrep + clippy.
check-static:
    @rg -n --pcre2 \
      '(TcpStream::connect|lookup_host|to_socket_addrs|reqwest|HttpConnector|ProviderBuilder::on_http|fetch\()' \
      src sidecar \
      | rg -v '^([^:]*):(\\s*//|\\s*\\*)' \
      && exit 1 || true
    cargo clippy --all-targets -- -D warnings

# L4. Spec-drift detector.
verify-spec:
    python3 scripts/verify_spec.py spec.md \
      --src src --sidecar sidecar \
      --cargo Cargo.toml --package-json sidecar/package.json \
      --claims claims.md

# L5. Sidecar sandbox + Railgun feasibility (gates M2).
check-sidecar:
    docker build -t undercover-sidecar:dev sidecar
    cargo test --test sidecar_permissions_smoke
    cargo run --release -- sidecar-smoke
    cargo run --release -- load-wallet-smoke --railgun-mnemonic \
      "test test test test test test test test test test test junk"

# L6. Event-sync correctness (gates M4).
check-event-sync:
    cargo test --release --test event_sync_root_match -- --nocapture

# L8. Stdout-discipline regression.
check-stdout:
    RUST_LOG=trace DENO_LOG=debug \
      cargo test --release --test stdout_discipline -- --nocapture

# Aggregate runs by cadence:
preflight: check-static check-deps verify-spec check-stdout
gate-m2: preflight check-sidecar
gate-m4: gate-m2 check-event-sync egress-audit-linux event-sync

ping rpc:
    cargo run --release -- ping --rpc {{rpc}}

sidecar-smoke:
    docker build -t undercover-sidecar:dev sidecar
    cargo run --release -- sidecar-smoke

shield-base-token amount_wei rpc="https://ethereum-sepolia-rpc.publicnode.com":
    docker build -t undercover-sidecar:dev sidecar
    cargo run --release -- shield-base-token --rpc {{rpc}} --amount-wei {{amount_wei}}

shield-base-token-dry-run amount_wei rpc="https://ethereum-sepolia-rpc.publicnode.com":
    docker build -t undercover-sidecar:dev sidecar
    cargo run --release -- shield-base-token --dry-run --rpc {{rpc}} --amount-wei {{amount_wei}}

signer-address:
    cargo run --release -- signer-address

demo rpc signer mnemonic shielded_to recipient token amount:
    cargo run --release -- demo \
        --rpc {{rpc}} \
        --signer-hex {{signer}} \
        --railgun-mnemonic {{mnemonic}} \
        --shielded-to {{shielded_to}} \
        --recipient-eoa {{recipient}} \
        --token {{token}} \
        --amount {{amount}}

# Captures all egress while running the supplied subcommand and asserts:
# (a) no DNS, (b) every TCP destination is in Tor's allow-set
# (directory authorities + fallback directories + current consensus
# relays), (c) no UDP egress to non-Tor destinations.
#
# Canonical setup is Linux: a fresh network namespace where this binary
# is the only thing routing, so packet attribution is structural. macOS
# is supported as a degraded mode (best-effort, no PID attribution) and
# is documented as such in the audit write-up.
egress-audit-linux *args:
    # The helper owns netns setup: create namespace, veth pair, host-side
    # NAT/routing, loopback, cleanup trap, and pcap capture inside the ns.
    sudo scripts/run_netns_audit.sh \
      --pcap trace.pcap -- cargo run --release -- {{args}}
    python3 scripts/audit_egress.py trace.pcap \
      --da-list ./scripts/tor_directory_authorities.json \
      --fallback-dirs ./scripts/tor_fallback_dirs.json \
      --consensus ./.arti/cache/dir/consensus-microdesc.txt

# Degraded macOS mode: cannot attribute packets to PID. Run on a host
# with no other Tor traffic active, accept that capture may include
# unrelated egress, and use this only for inner-loop iteration. The
# milestone gate uses egress-audit-linux.
egress-audit-macos *args:
    sudo tcpdump -i any -w trace.pcap not host 127.0.0.1 &
    PID=$!; sleep 1; \
      cargo run --release -- {{args}}; \
      sudo kill $PID
    python3 scripts/audit_egress.py trace.pcap \
      --da-list ./scripts/tor_directory_authorities.json \
      --fallback-dirs ./scripts/tor_fallback_dirs.json \
      --consensus ./.arti/cache/dir/consensus-microdesc.txt \
      --degraded-attribution

fmt:
    cargo fmt
```

Acceptance checks (run in order):

1. `just build` succeeds. `cargo deny check bans` passes (no `reqwest`
   anywhere). `cargo clippy --all-targets -- -D warnings` passes (no
   disallowed-method violations).

2. `just ping https://<rpc>`: prints chain id and a recent block
   number.

3. `just egress-audit-linux`: the canonical network-isolation check.
   This is the milestone gate. macOS hosts use `egress-audit-macos`
   for fast iteration but it is not the gate.

   Methodology:

   - **Linux (canonical).** Run inside a fresh network namespace so
     packet attribution is structural: the only process routing in
     the ns is `undercover`, so every packet captured is by
     definition this binary's egress. No PID-based filtering is
     needed.
   - **macOS (degraded).** `tcpdump` cannot attribute by PID. Run on
     a host with no other Tor traffic active. `audit_egress.py`'s
     `--degraded-attribution` flag emits a warning that capture
     hygiene is the operator's responsibility. For serious audits,
     run inside a Linux VM.
   - In both modes, post-process `trace.pcap` with
     `scripts/audit_egress.py`, which applies three assertions and
     exits non-zero on any violation:
       a. **No DNS.** Zero packets with destination port 53 (UDP or
          TCP) and zero mDNS / LLMNR traffic. Tor resolves names
          inside the circuit; OS resolvers must never be invoked.
       b. **All TCP destinations are in Tor's allow-set.** Every
          outbound TCP SYN's destination IP is in the union of:
            - Tor directory authorities (small, hardcoded; pinned in
              `scripts/tor_directory_authorities.json`, sourced from
              Arti's `tor-dirmgr` defaults).
            - Tor fallback directories (pinned in
              `scripts/tor_fallback_dirs.json`, sourced from Arti's
              fallback list snapshot at the time of audit).
            - Current consensus relays (read from
              `./.arti/cache/dir/consensus-microdesc.txt`).
          The DA + fallback set is required because during cold-cache
          bootstrap, before any consensus is loaded, all directory
          traffic legitimately goes to DAs and fallback dirs. A
          relay-only allow-list would falsely fail the bootstrap
          phase. Any IP outside the union is a violation regardless
          of port.
       c. **No QUIC/UDP egress to non-Tor.** Zero outbound UDP
          packets to any destination outside the allow-set above.
   - The pcap snapshots and the allow-set source files are checked
     into the repo for the audit write-up at M7, so any reviewer can
     replay the analysis offline.

4. `just artifacts`: downloads Railgun proving keys to `./artifacts`.

5. `just sidecar-smoke`: returns
   `{"sdk_version":"...","node_compat":true}`. Confirms
   `@railgun-community/wallet` resolves cleanly under Node in the
   Docker sidecar.

6. `just demo ...`: prints up to three transaction hashes (shield,
   transfer, unshield). Each should be visible on the chosen
   testnet's explorer; the shield and unshield show interactions
   with the Railgun contract, the transfer shows a pure shielded
   operation.

7. `just egress-audit-linux demo`: combines (3) with (6). Must pass
   assertions a, b, c throughout the full shield-transfer-unshield
   run. This is the canonical evidence that the spec was implemented
   as written.

## Verification loops

Implementation is gated by nine layered loops. Each loop is one
command, exits non-zero on failure, and gates a specific class of
regression. The loops are organized by cadence (fastest first) and
ordered by risk (highest-risk unknowns front-loaded). The agent runs
the inner loops continuously and the outer loops at milestone
boundaries.

The principle: every privacy claim must be either statically
enforced (banned methods, banned crates, feature-graph greps) or
runtime-witnessed (egress audit, deny-net probe, connector-call
counter). Documentation alone is not verification.

### L1. Dependency / feature-graph hygiene

Cadence: every `Cargo.toml`, `sidecar/package.json`, or lockfile change
(~3 seconds).

```
just check-deps
```

Runs:

- `cargo build`.
- `cargo tree -e features | rg
   '(reqwest|alloy-transport-http|HttpConnector|hyper-tls|native-tls)'`
   and asserts the grep finds **zero** matches. This is the primary
   safeguard: `cargo deny` only catches what we remember to ban,
   while the feature-graph grep catches the known footguns regardless
   of how they entered the graph (transitive deps, feature unification,
   silent feature additions in a `cargo update`).
- `cargo deny check bans`.
- `cargo fmt --check`.
- A weekly cron runs `cargo update` then re-runs the above. A patch
  bump that reintroduces `reqwest` or `alloy-transport-http`
  transitively must fail this cron, not slip in silently.

Catches: I1 / I8 violations, banned crates, transitive feature
sprouts.

### L2. Transport-spike call counter

Cadence: after every change to `src/transport.rs`, `src/rpc.rs`, or
the alloy/hyper feature set (~2 seconds).

The first implementation milestone is the `examples/spike.rs` file
defined in M1a. The spike's `ArtiConnector::call` increments a
process-wide `arti_connect_calls` counter via
`tracing::Span::record`. After one provider call (`eth_chainId` or
`eth_blockNumber`) returns, an integration test asserts the counter
is ≥ 1. If a provider request succeeds without hitting
`ArtiConnector::call`, the test fails.

This is the positive runtime proof: not "no clearnet packets" (an
absence-proof), but "the connector was actually used" (a presence-
proof). A bug that silently zeroed traffic would pass the egress
audit and fail this loop.

```
just spike
```

Runs the spike binary plus its assertion test; gates M1a.

### L3. Static no-clearnet ripgrep + clippy

Cadence: every save (~1 second).

```
just check-static
```

Runs:

- `rg -n --pcre2
   '(TcpStream::connect|lookup_host|to_socket_addrs|reqwest|HttpConnector|ProviderBuilder::on_http|fetch\()'
   src sidecar` and asserts zero matches outside an explicit
   allow-list (e.g. `src/transport.rs` may name `lookup_host` only
   in a doc comment explaining why it is forbidden).
- `cargo clippy --all-targets -- -D warnings` with the
  `disallowed_methods` from § Components.

Belt-and-suspenders: clippy enforces the structural ban, ripgrep
catches accidental introductions in places clippy might miss
(e.g., string literals in tests, doc comments that drift toward
copy-paste examples).

### L4. Spec-drift detector

Cadence: when `spec.md` or any `src/`, `sidecar/`, `Cargo.toml`,
or `sidecar/package.json` changes (~2 seconds).

```
just verify-spec
```

A `scripts/verify_spec.py` parses `spec.md` and asserts the code
matches the spec verbatim:

- Every dependency named in § Components matches `Cargo.toml`.
- Every banned method or crate appears in `clippy.toml` /
  `deny.toml` and in the L3 ripgrep pattern.
- Every JSON-RPC method named in § Components has a Rust caller
  (in `src/sidecar.rs` or `src/flow.rs`) and a Node handler (in
  `sidecar/main.mjs`).
- Every module path in § Components exists in `src/`.
- The exact Docker spawn argv string in § Node sidecar container matches the argv
  constant in `Sidecar::spawn`.
- Every invariant I1-I9 cites at least one passing loop or test
  (the claim ledger, see end of section).

Drift is the most insidious failure mode in security work: spec
says one thing, code drifts to another, both pass their own tests.
This loop closes that gap mechanically.

### L5. Sidecar sandbox + Railgun SDK feasibility

Cadence: after every `sidecar/main.mjs` change; gates M2.

```
just check-sidecar
```

Runs the container permission-matrix probe, `sidecar-smoke`, and
`load-wallet-smoke`.

L5a: A `sidecar-permissions-smoke` JSON-RPC method that intentionally
attempts each forbidden operation and asserts each fails:

- `fetch("https://example.com")` → expect failure.
- `node:net` connect to `1.1.1.1:53` → expect failure.
- `node:net` connect to a host loopback listener → expect failure.
- `fs.writeFile(...)` → expect failure.
- `process.env.UNDERCOVER_FORBIDDEN_ENV` → expect absent.

Plus a positive control: `fs.readFile("/app/artifacts/manifest")`
must succeed. Without the positive control, a runtime that
deny-everythinged would pass the negative tests silently.

This loop is run before any orchestration code is written. It
front-loads the highest-leverage architectural risk in the PoC.

### L6. Event-sync correctness

Cadence: before any transaction-submission code is written; gates
M4.

```
just check-event-sync
```

A deterministic fixture-based test:

- Rust fetches a known block range through Arti from the chosen
  testnet (the range is small enough to be a checked-in fixture
  if we cache the responses).
- The sidecar ingests the fetched logs.
- The sidecar's `merkle_root` is compared against an `eth_call`-
  fetched on-chain root for the same block.
- Roots must match exactly. If they don't, the sync is wrong, and
  no transaction-submission code is written until they agree.

This loop catches "the SDK silently mis-scanned" before it
manifests as a transaction-revert in M5 or M6.

### L7. Egress audit (the final gate)

Cadence: per phase, per milestone (M1a, M4-M7); ~minutes.

```
just egress-audit-linux <subcommand>
```

Mechanics in § Build, run, verify, acceptance check 3.

**Hierarchy:** tracing logs (`arti_connect_calls`, circuit IDs,
phase labels) are useful for fast inner-loop iteration but are
**not** the proof. Tracing-green is necessary but not sufficient.
Pcap-green is the milestone gate. The risk of treating pcap as
optional is that devs skip it because it is slower; treat it as the
canonical evidence and run the cheaper tracing checks as a preview.

### L8. Stdout-discipline regression

Cadence: per commit (~5 seconds).

```
just check-stdout
```

Runs a 100-method sequence against the sidecar with `RUST_LOG=trace`
and `DENO_LOG=debug`. Captures the sidecar's stdout. Asserts every
line parses as a JSON-RPC request or response. A stray `console.log`
from a transitive npm dep, or a `tracing` output that escaped to
stdout instead of stderr, fails this loop.

This loop catches a bug class that would otherwise manifest as
flaky integration tests months into the project.

### L9. Adversarial mutation probes

Cadence: per milestone, manual.

A `mutations/` directory holds patches that are designed to break a
specific security claim. The agent applies each patch in turn, runs
the loops it expects to fail, asserts they fail with the expected
diagnostic, and reverts. If a mutation passes a loop it was meant to
break, the loop is broken (not the mutation), and the loop is fixed
before any other work proceeds.

Required mutations:

- **M-A.** Replace `TorClient::connect` with `TcpStream::connect` in
  `transport.rs`. Expectation: L3 (clippy) fails.
- **M-B.** Drop `--network none` from the Docker spawn argv.
  Expectation: L4 (spec drift) fails because argv no longer matches;
  L5a also fails because a network call now succeeds.
- **M-C.** Add `reqwest = "0.12"` to `Cargo.toml`. Expectation: L1
  (cargo tree grep) fails; `cargo deny` also fails as a backup.
- **M-D.** Bypass `isolated_client` and reuse one client across all
  phases. Expectation: L2's circuit-distinctness test (which logs
  circuit IDs per phase) fails.
- **M-E.** Have the sidecar emit one stray `console.log("startup ok")`
  on stdout. Expectation: L8 fails.
- **M-F.** Remove `default-features = false` from any alloy sub-crate
  (or revert to the meta-crate). Expectation: L1 fails because
  `alloy-transport-http` reappears in the feature graph.
- **M-G.** Pin a stale guard state in `./.arti/state` from a
  retired-relay snapshot. Expectation: L7 (egress audit) shows
  TCP destinations not in the current Tor allow-set.

This loop is the only way to know the other loops actually catch
what they claim to catch.

### Running order at each cadence

Tight inner loop (after every edit): L1, L3.
Before any commit: L1, L3, L4, L8 (~10 seconds total).
At M1a: spike + L1 + L2 + L7 (~minutes).
At M2 boundary: full L1-L5; Q1 + Q5 + Q6 must close before M2 closes.
At M3 boundary: full L1-L5.
At M4 boundary: full L1-L8 including L6.
At M5/M6 boundaries: full L1-L8.
Manual cadence (per milestone after M2): L9 mutation probes.

### The claim ledger

A `claims.md` checked into the repo maps every invariant I1-I9 and
every milestone gate to the exact loop, test, or check that
witnesses it. Format:

```
- I1 (all TCP egress through Arti):
    static: L1 (cargo tree grep), L3 (clippy + ripgrep)
    runtime: L2 (connector counter), L7 (egress audit)
    mutation: M-A, M-G

- I3 (split key custody): ...
- M5 (shield works): L1, L4, L6, L7, plus on-chain receipt assertion
- ...
```

The rule: the agent may not write "done" against any milestone
whose claims do not all cite a passing loop. Claims without checks
are red-flagged; the agent must either add the check or remove the
claim from the spec. L4 enforces this mechanically.

## Open questions

Q1. **Does the Railgun SDK load in the sidecar boundary?** Resolved
for the PoC. The active boundary is Node 22 inside Docker, not Deno.
`sidecar-smoke` returns `node_compat=true`; `load-wallet-smoke`
initializes a Railgun wallet and returns a 0zk address; the permission
smoke confirms the container blocks `fetch`, public sockets, loopback
`node:net`, writes, and broad env access while allowing artifact reads.

The earlier Deno experiment remains useful evidence: Deno's permission
model did not reliably constrain transitive `node:net` usage for this
SDK shape, so Deno is not the current PoC runtime.

Q2. **Public Railgun broadcaster vs self-broadcast for the private
transfer?** PoC default: self-broadcast through Arti. Simpler, keeps
the egress audit trivial. Broadcaster integration gives a larger
anonymity set but introduces a third-party dependency that must also
be reachable through Arti and that learns metadata about transfer fees.
Tracked as a follow-up after the PoC clears M6.

Q4. **Provider load balancing.** PoC targets one RPC endpoint.
Distributing requests across multiple endpoints over distinct
circuits is a follow-up.

Q5. **Is Sepolia actively supported by Railgun's deployed contracts
and SDK constants today?** Railgun's public materials emphasize
Ethereum, Polygon, BSC, and Arbitrum as live networks. Testnet
deployments are tracked in Railgun's deployments registry; the SDK's
`NetworkName` enum is the source of truth for which testnets the SDK
will actually accept. Action before M2:

  1. Read the SDK's `NetworkName` constants and verify Sepolia
     (`EthereumSepolia`) is present.
  2. Pull the Railgun deployments file and confirm contract addresses
     for Sepolia.
  3. If absent, retarget to whichever testnet is supported (e.g.
     Polygon Amoy or Arbitrum Sepolia). The architecture, JSON-RPC
     contract, and security invariants are network-agnostic; only the
     RPC URL, chain id, and contract addresses change.

Q6. **How does the Railgun SDK ingest external event batches?**
This is the largest remaining feasibility risk in the spec; if the
SDK cannot be driven with externally supplied logs, the JSON-RPC
contract changes substantially. **Resolution is a precondition of
M2, not M3 or M4.**

The PoC's data flow currently assumes the SDK exposes an API to feed
it already-fetched logs (so the sidecar never touches the network).
If the SDK insists on owning its provider, the fallback is a stdio
provider shim: the sidecar implements an ethers/viem-compatible
provider whose `send()` writes a JSON-RPC request to stdout in a
distinct envelope (e.g. `{"reverse_id":N,"rpc":{...}}`), the Rust
side reads such envelopes, executes them via Arti, and replies on
stdin. The protocol becomes bidirectional rather than strict
request/response. Slower (round-trip per RPC) and changes
`src/sidecar.rs` materially.

Resolution path after the current shield PoC: decide whether private
transfer/unshield uses direct event ingestion or the stdio provider
shim. The shield-base-token path already avoids this issue because it
only asks the SDK to populate calldata and uses Rust for broadcast.

Q7. **Stronger sidecar sandbox.** Docker `--network none` is enough
for this PoC. A stronger boundary (`unshare -n`, a narrower container
profile, macOS `sandbox-exec`, or a pf anchor) is a follow-up after
M6 and a prerequisite for any production-adjacent use.

## Out of scope

- Persistent encrypted seed storage. PoC reads a hex private key from a
  CLI flag.
- Mainnet deployment.
- Browser or extension target.
- Any modification to Railgun's circuits or proof system.
- A user interface beyond a CLI.
- Recovery flows for lost shielded notes.

## Milestones

M1. **Compile-and-request spike, then build hygiene + Arti.** This
is two sub-milestones because the construction story is
load-bearing for everything else.

  M1a. **Spike (single file, before any module scaffolding).** Write
  `examples/spike.rs` that constructs the exact Arti + hyper +
  alloy stack the spec calls for, and makes one `eth_blockNumber`
  call to a known testnet RPC. The spike must:
    - Compile.
    - `cargo tree -e features` of the spike must contain neither
      `alloy-transport-http` nor `reqwest`.
    - The L2 connector counter inside `ArtiConnector::call` must
      show ≥ 1 hit during the call.
    - A pcap captured during the run must satisfy
      `audit_egress.py` (Tor allow-set only).
  The spike is the gate that says "the central technical hypothesis
  of this PoC compiles and runs." If any of those four assertions
  fails, the rest of the spec is paused until the construction is
  reworked. No further module code is written until M1a passes.

  M1b. **Module scaffolding.** With M1a's construction story
  validated, scaffold `src/arti.rs`, `src/transport.rs`,
  `src/rpc.rs`, `src/sidecar.rs`, `src/flow.rs`, `src/error.rs`,
  `src/main.rs` with the public signatures from § Components.
  `cargo deny check bans` passes (no `reqwest`).
  `cargo clippy --all-targets -- -D warnings` passes. Arti
  bootstraps with persistent state. `eth_blockNumber` succeeds via
  the production `ArtiConnector`-backed transport (not the spike).
  Per-phase isolated clients are obtained via `isolated_client` and
  verified to produce distinct circuits (smoke test logs Tor
  circuit IDs).

M2. **Network selection + sidecar smoke + Railgun feasibility (Q1 +
Q5 + Q6 closed).** SDK's `NetworkName` constants and Railgun's
deployments registry are checked first. The active PoC target is
Ethereum Sepolia through the SDK's `Ethereum_Sepolia` constants. The
Node sidecar image builds from `sidecar/Dockerfile`, spawns under the
Docker argv in `src/sidecar.rs`, `health` round-trips via stdio,
`load_wallet` returns a 0zk address, and `populate_shield_base_token`
returns V2 RelayAdapt calldata for Sepolia. The sidecar network/write
permission smoke passes under Docker.

M3. **Artifacts loaded against the selected chain.** `load_artifacts`
succeeds with proving keys downloaded by `fetch_artifacts.ts`.
Sidecar chain IDs and contract addresses used by `load_wallet`,
`ingest_events`, and `build_*` match `chains.toml`.

M4. **Wallet sync end-to-end.** `load_wallet` with a fresh test
mnemonic returns a 0zk address. Phase 0 event sync runs to chain head
on the chosen testnet via Arti. The sidecar's `merkle_root` matches
the on-chain root after sync. No clearnet egress observed during sync
(audit_egress.py passes).

M5. **Shield.** Shield flow works end to end. A testnet transaction
succeeds. Subsequent incremental sync shows the new note in the
sidecar. `audit_egress.py` passes for the full phase.

M6. **Private transfer + unshield.** Both flows work end to end on
the chosen testnet. The transfer's on-chain tx is a Railgun shielded
transfer; the unshield credits a fresh EOA. `audit_egress.py` passes
for both phases.

M7. **Egress audit write-up.** Documents the audit_egress.py
methodology, what an external observer sees during a full
sync-shield-transfer-unshield run, and the residual leakage surface
(timing and amount correlation between shield and unshield, behavior
of the chosen RPC endpoint under repeated Tor-circuit access, fee
fingerprinting). Closes the PoC.
