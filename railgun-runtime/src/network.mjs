// Network-side plumbing: the reverse-RPC bridge to Rust, the Tor-backed
// EIP-1193 / Ethers provider, the Railgun engine bootstrap, and quick-sync
// helpers.

import * as wallet from "@railgun-community/wallet";
import * as sharedModels from "@railgun-community/shared-models";
import { BrowserProvider, Network } from "ethers";
import { Buffer } from "node:buffer";

import { artifactStore } from "./artifacts.mjs";
import {
  op_hermetic_reverse_request,
  op_hermetic_service_endpoint,
  trace,
} from "./host-ops.mjs";
import { groth16 } from "./snark.mjs";
import { SqliteLevelDOWN } from "./storage.mjs";

export const NETWORK_NAME = sharedModels.NetworkName.EthereumSepolia;
export const TXID_VERSION = sharedModels.TXIDVersion.V2_PoseidonMerkle;
export const BASE_TOKEN_ADDRESS =
  sharedModels.BaseTokenWrappedAddress[NETWORK_NAME];

const serviceEndpoints = {
  graphql: new URL(op_hermetic_service_endpoint("graphql")),
  poi: new URL(op_hermetic_service_endpoint("poi")),
};
const poiNodeURLs = [serviceEndpoints.poi.toString()];

let engineStarted = false;
let networkLoaded = false;
let localFilterID = 1;
const localFilters = new Map();

function endpointService(url) {
  for (const [service, endpoint] of Object.entries(serviceEndpoints)) {
    if (url.origin === endpoint.origin) {
      return service;
    }
  }
  throw new Error(`Railgun SDK requested non-hosted URL: ${url.origin}`);
}

export async function hostServiceFetch(input, init = {}) {
  const inputUrl = typeof input === "string" ? input : input.url;
  const url = new URL(inputUrl);
  const service = endpointService(url);
  const method = init.method ??
    (typeof input === "string" ? "GET" : input.method) ?? "GET";
  const headers = new Headers(
    typeof input === "string" ? undefined : input.headers,
  );
  new Headers(init.headers).forEach((value, key) => headers.set(key, value));
  const body = init.body == null ? undefined : Buffer.from(
    typeof init.body === "string"
      ? init.body
      : await new Response(init.body).arrayBuffer(),
  ).toString("base64");
  const response = await op_hermetic_reverse_request({
    kind: "service_http",
    service,
    path: `${url.pathname}${url.search}`,
    method,
    headers: Array.from(headers.entries()),
    body_base64: body,
  });
  return new Response(Buffer.from(response.body_base64, "base64"), {
    status: response.status,
    headers: response.headers,
  });
}

// Side effects: redirect SDK fetch calls into the host op. Both globalThis
// (for the SDK's modern fetch path) and the @whatwg-node/fetch shim (for
// graphql-mesh-derived plumbing) need patching.
globalThis.fetch = hostServiceFetch;
require("@whatwg-node/fetch").fetch = hostServiceFetch;

function reverseRpc(method, params = []) {
  return op_hermetic_reverse_request({
    kind: "json_rpc",
    method,
    params,
  });
}

function createTorEip1193Provider() {
  return {
    request: async ({ method, params }) => {
      const requestParams = params ?? [];
      if (method === "eth_newFilter") {
        const id = `0xhermetic${localFilterID++}`;
        const latestBlock = BigInt(await reverseRpc("eth_blockNumber", []));
        localFilters.set(id, {
          filter: requestParams[0] ?? {},
          nextBlock: latestBlock + 1n,
        });
        return id;
      }
      if (method === "eth_getFilterChanges") {
        const id = requestParams[0];
        const state = localFilters.get(id);
        if (!state) {
          return [];
        }
        const latestBlock = BigInt(await reverseRpc("eth_blockNumber", []));
        if (state.nextBlock > latestBlock) {
          return [];
        }
        const logs = await reverseRpc("eth_getLogs", [
          {
            ...state.filter,
            fromBlock: `0x${state.nextBlock.toString(16)}`,
            toBlock: `0x${latestBlock.toString(16)}`,
          },
        ]);
        state.nextBlock = latestBlock + 1n;
        return logs;
      }
      if (method === "eth_uninstallFilter") {
        return localFilters.delete(requestParams[0]);
      }
      return reverseRpc(method, requestParams);
    },
  };
}

function createTorEthersProvider() {
  const network = Network.from(11155111);
  const provider = new BrowserProvider(createTorEip1193Provider(), network, {
    staticNetwork: network,
  });
  provider.isPollingProvider = true;
  return provider;
}

export async function ensureEngine() {
  if (engineStarted) {
    return;
  }
  wallet.POINodeRequest.jsonRpcRequest = async (url, method, params) => {
    const response = await hostServiceFetch(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        method,
        params,
        id: Date.now(),
      }),
    });
    const body = await response.json();
    if (!response.ok || body.error) {
      throw new Error(
        `POI request failed: status=${response.status} body=${
          JSON.stringify(body)
        }`,
      );
    }
    return body.result;
  };
  await wallet.startRailgunEngine(
    "hermetic",
    new SqliteLevelDOWN(),
    false,
    artifactStore,
    false,
    false,
    poiNodeURLs,
    undefined,
    false,
  );
  wallet.getEngine().prover.setSnarkJSGroth16(groth16);
  engineStarted = true;
}

export async function ensureNetworkLoadedThroughTor() {
  await ensureEngine();
  if (networkLoaded) {
    return;
  }
  const network = sharedModels.NETWORK_CONFIG[NETWORK_NAME];
  const provider = createTorEthersProvider();
  wallet.setFallbackProviderForNetwork(NETWORK_NAME, provider);
  wallet.setPollingProviderForNetwork(NETWORK_NAME, provider);
  await wallet.getEngine().loadNetwork(
    network.chain,
    network.proxyContract,
    network.relayAdaptContract,
    network.poseidonMerkleAccumulatorV3Contract,
    network.poseidonMerkleVerifierV3Contract,
    network.tokenVaultV3Contract,
    provider,
    provider,
    {
      [TXID_VERSION]: network.deploymentBlock ?? 0,
      [sharedModels.TXIDVersion.V3_PoseidonMerkle]:
        network.deploymentBlockPoseidonMerkleAccumulatorV3 ?? 0,
    },
    network.poi?.launchBlock,
    network.supportsV3,
  );
  networkLoaded = true;
}

async function quickSyncWalletState(chain) {
  const engine = wallet.getEngine();
  const network = sharedModels.NETWORK_CONFIG[NETWORK_NAME];
  const startingBlock = network.deploymentBlock ?? 0;
  const { commitmentEvents, unshieldEvents, nullifierEvents } = await engine
    .quickSyncEvents(TXID_VERSION, chain, startingBlock);
  trace(
    `quick sync events commitments=${commitmentEvents.length} unshields=${unshieldEvents.length} nullifiers=${nullifierEvents.length}`,
  );
  await engine.unshieldListener(TXID_VERSION, chain, unshieldEvents);
  await engine.nullifierListener(TXID_VERSION, chain, nullifierEvents);
  await engine.commitmentListener(
    TXID_VERSION,
    chain,
    commitmentEvents,
    false,
    false,
  );
  if (commitmentEvents.length !== 0) {
    await engine.getUTXOMerkletree(TXID_VERSION, chain)
      .updateTreesFromWriteQueue(true);
  }
  await engine.syncRailgunTransactionsV2(chain, "hermetic balance refresh");
}

export async function refreshSdkBalanceState(
  chain,
  walletID,
  { requirePoiRefresh = false } = {},
) {
  const engine = wallet.getEngine();
  await quickSyncWalletState(chain);
  await engine.decryptBalancesAllWallets(
    TXID_VERSION,
    chain,
    [walletID],
    undefined,
    true,
  );
  try {
    await wallet.refreshReceivePOIsForWallet(
      TXID_VERSION,
      NETWORK_NAME,
      walletID,
    );
  } catch (error) {
    if (requirePoiRefresh) {
      throw error;
    }
    trace(
      `POI refresh skipped after failure: ${String(error?.stack ?? error)}`,
    );
  }
}
