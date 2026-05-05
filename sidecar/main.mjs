import * as wallet from '@railgun-community/wallet';
import * as sharedModels from '@railgun-community/shared-models';
import memdown from 'memdown';
import net from 'node:net';
import fs from 'node:fs/promises';
import readline from 'node:readline';
import path from 'node:path';
import { createRequire } from 'node:module';
import { BrowserProvider, Network } from 'ethers';

let engineStarted = false;
let networkContractsLoaded = false;
let reverseRpcID = 1;
let localFilterID = 1;
const pendingReverseRpc = new Map();
const localFilters = new Map();
let reverseHttpEnabled = false;
const poiNodeURLs = ['https://ppoi-agg.horsewithsixlegs.xyz'];

const require = createRequire(import.meta.url);
const engineDist = path.dirname(require.resolve('@railgun-community/engine'));
const { ContractStore } = require(path.join(
  engineDist,
  'contracts/contract-store.js',
));
const { RelayAdaptV2Contract } = require(path.join(
  engineDist,
  'contracts/relay-adapt/V2/relay-adapt-v2.js',
));
const walletDist = path.dirname(require.resolve('@railgun-community/wallet'));
const { print } = require('graphql');
const { getEngine } = require(path.join(
  walletDist,
  'services/railgun/core/engine.js',
));
const { POINodeRequest } = require(path.join(
  walletDist,
  'services/poi/poi-node-request.js',
));
const axios = require('axios');
const snarkjs = require('snarkjs');
const graphV2 = require(path.join(
  walletDist,
  'services/railgun/quick-sync/V2/graphql/index.js',
));
const graphFormattersV2 = require(path.join(
  walletDist,
  'services/railgun/quick-sync/V2/graph-type-formatters-v2.js',
));
const graphQuery = require(path.join(
  walletDist,
  'services/railgun/quick-sync/graph-query.js',
));

function artifactPath(relativePath) {
  const resolved = path.resolve('/app/artifacts', relativePath);
  if (!resolved.startsWith('/app/artifacts/')) {
    throw new Error(`invalid artifact path: ${relativePath}`);
  }
  return resolved;
}

const artifactStore = new wallet.ArtifactStore(
  (relativePath) => {
    const resolved = artifactPath(relativePath);
    return fs
      .readFile(resolved, relativePath.endsWith('.json') ? 'utf8' : undefined)
      .catch(() => null);
  },
  async (dir, relativePath, item) => {
    await fs.mkdir(artifactPath(dir), { recursive: true });
    await fs.writeFile(artifactPath(relativePath), item);
  },
  (relativePath) =>
    fs
      .access(artifactPath(relativePath))
      .then(() => true)
      .catch(() => false),
);

axios.get = async (url, options = {}) => {
  const response = await reverseHttpFetch(url, {
    method: options.method ?? 'GET',
    headers: {},
  });
  const body =
    options.responseType === 'arraybuffer'
      ? Buffer.from(await response.arrayBuffer())
      : await response.text();
  if (!response.ok) {
    throw new Error(`HTTP ${response.status} fetching ${url}`);
  }
  return { data: body };
};

async function ensureEngine() {
  if (engineStarted) {
    return;
  }
  POINodeRequest.jsonRpcRequest = async (url, method, params) => {
    const response = await reverseHttpFetch(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        jsonrpc: '2.0',
        method,
        params,
        id: Date.now(),
      }),
    });
    const body = await response.json();
    if (!response.ok || body.error) {
      throw new Error(
        `POI request failed: status=${response.status} body=${JSON.stringify(body)}`,
      );
    }
    return body.result;
  };
  await wallet.startRailgunEngine(
    'undercover',
    memdown(),
    false,
    artifactStore,
    false,
    false,
    poiNodeURLs,
    undefined,
    false,
  );
  getEngine().prover.setSnarkJSGroth16(snarkjs.groth16);
  engineStarted = true;
}

async function directGraphQL(query, variables) {
  const response = await reverseHttpFetch(
    'https://rail-squid.squids.live/squid-railgun-eth-sepolia-v2/graphql',
    {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ query: print(query), variables }),
    },
  );
  const body = await response.json();
  if (!response.ok || body.errors) {
    throw new Error(
      `quick-sync GraphQL failed: status=${response.status} body=${JSON.stringify(body)}`,
    );
  }
  return body.data;
}

function createGraphCommitmentBatches(flattenedCommitments) {
  const graphCommitmentMap = {};
  for (const commitment of flattenedCommitments) {
    const startPosition = commitment.batchStartTreePosition;
    const treeNumber = commitment.treeNumber;
    const key = `${treeNumber}-${startPosition}`;
    const existingBatch = graphCommitmentMap[key];
    if (existingBatch) {
      existingBatch.commitments.push(commitment);
    } else {
      graphCommitmentMap[key] = {
        commitments: [commitment],
        transactionHash: commitment.transactionHash,
        treeNumber: commitment.treeNumber,
        startPosition: commitment.batchStartTreePosition,
        blockNumber: Number(commitment.blockNumber),
      };
    }
  }
  return Object.values(graphCommitmentMap).filter(Boolean);
}

function sortByTreeNumberAndStartPosition(a, b) {
  if (a.treeNumber !== b.treeNumber) {
    return a.treeNumber < b.treeNumber ? -1 : 1;
  }
  if (a.startPosition !== b.startPosition) {
    return a.startPosition < b.startPosition ? -1 : 1;
  }
  return 0;
}

async function directQuickSyncEvents(txidVersion, chain, startingBlock) {
  const network = sharedModels.networkForChain(chain);
  if (
    txidVersion !== sharedModels.TXIDVersion.V2_PoseidonMerkle ||
    network?.name !== sharedModels.NetworkName.EthereumSepolia
  ) {
    return graphQuery.EMPTY_EVENTS;
  }

  const maxQueryResults = 16 * 2 ** 16;
  const block = startingBlock.toString();
  process.stderr.write(`direct quick-sync V2 Sepolia from block ${block}\n`);
  const nullifiers = await graphQuery.autoPaginatingQuery(
    async (blockNumber) =>
      (await directGraphQL(graphV2.NullifiersDocument, { blockNumber })).nullifiers,
    block,
    maxQueryResults,
  );
  const unshields = await graphQuery.autoPaginatingQuery(
    async (blockNumber) =>
      (await directGraphQL(graphV2.UnshieldsDocument, { blockNumber })).unshields,
    block,
    maxQueryResults,
  );
  const commitments = await graphQuery.autoPaginatingQuery(
    async (blockNumber) =>
      (await directGraphQL(graphV2.CommitmentsDocument, { blockNumber })).commitments,
    block,
    maxQueryResults,
  );
  const commitmentBatches = createGraphCommitmentBatches(commitments);
  commitmentBatches.sort(sortByTreeNumberAndStartPosition);
  process.stderr.write(
    `direct quick-sync results nullifiers=${nullifiers.length} unshields=${unshields.length} commitments=${commitments.length}\n`,
  );
  return {
    nullifierEvents: graphFormattersV2.formatGraphNullifierEventsV2(nullifiers),
    unshieldEvents: graphFormattersV2.formatGraphUnshieldEventsV2(unshields),
    commitmentEvents: graphFormattersV2.formatGraphCommitmentEventsV2(commitmentBatches),
  };
}

function ensureNetworkContracts() {
  if (networkContractsLoaded) {
    return;
  }
  const networkName = sharedModels.NetworkName.EthereumSepolia;
  const { chain, relayAdaptContract } = sharedModels.NETWORK_CONFIG[networkName];
  ContractStore.relayAdaptV2Contracts.set(
    null,
    chain,
    new RelayAdaptV2Contract(relayAdaptContract),
  );
  networkContractsLoaded = true;
}

function respond(response) {
  process.stdout.write(
    `${JSON.stringify(response, (_key, value) =>
      typeof value === 'bigint' ? value.toString() : value,
    )}\n`,
  );
}

function reverseRpc(method, params = []) {
  const id = reverseRpcID++;
  const request = {
    undercover_reverse_rpc: true,
    id,
    method,
    params,
  };
  process.stdout.write(`${JSON.stringify(request)}\n`);
  return new Promise((resolve, reject) => {
    pendingReverseRpc.set(id, { resolve, reject });
  });
}

async function reverseHttpFetch(input, init = {}) {
  if (!reverseHttpEnabled) {
    throw new Error('sidecar fetch is disabled');
  }
  const url = typeof input === 'string' ? input : input.url;
  const method = init.method ?? (typeof input === 'string' ? 'GET' : input.method);
  const headers = new Headers(typeof input === 'string' ? undefined : input.headers);
  new Headers(init.headers).forEach((value, key) => headers.set(key, value));
  const body =
    init.body == null
      ? undefined
      : Buffer.from(
          typeof init.body === 'string' ? init.body : await new Response(init.body).arrayBuffer(),
        ).toString('base64');
  const response = await reverseRpc('__http_request', {
    url,
    method,
    headers: Array.from(headers.entries()),
    body_base64: body,
  });
  return new Response(Buffer.from(response.body_base64, 'base64'), {
    status: response.status,
    headers: response.headers,
  });
}

globalThis.fetch = reverseHttpFetch;

function handleReverseRpcResponse(response) {
  const id = typeof response.id === 'number' ? response.id : null;
  const pending = pendingReverseRpc.get(id);
  if (!pending) {
    return false;
  }
  pendingReverseRpc.delete(id);
  if (response.error) {
    pending.reject(new Error(String(response.error)));
  } else {
    pending.resolve(response.result);
  }
  return true;
}

function createArtiEip1193Provider() {
  return {
    request: async ({ method, params }) => {
      const requestParams = params ?? [];
      if (method === 'eth_newFilter') {
        const id = `0xundercover${localFilterID++}`;
        const latestBlock = BigInt(await reverseRpc('eth_blockNumber', []));
        localFilters.set(id, {
          filter: requestParams[0] ?? {},
          nextBlock: latestBlock + 1n,
        });
        return id;
      }
      if (method === 'eth_getFilterChanges') {
        const id = requestParams[0];
        const state = localFilters.get(id);
        if (!state) {
          return [];
        }
        const latestBlock = BigInt(await reverseRpc('eth_blockNumber', []));
        if (state.nextBlock > latestBlock) {
          return [];
        }
        const logs = await reverseRpc('eth_getLogs', [
          {
            ...state.filter,
            fromBlock: `0x${state.nextBlock.toString(16)}`,
            toBlock: `0x${latestBlock.toString(16)}`,
          },
        ]);
        state.nextBlock = latestBlock + 1n;
        return logs;
      }
      if (method === 'eth_uninstallFilter') {
        return localFilters.delete(requestParams[0]);
      }
      return reverseRpc(method, requestParams);
    },
  };
}

function createArtiEthersProvider() {
  const network = Network.from(11155111);
  const provider = new BrowserProvider(createArtiEip1193Provider(), network, {
    staticNetwork: network,
  });
  provider.isPollingProvider = true;
  return provider;
}

async function ensureNetworkLoadedThroughArti() {
  await ensureEngine();
  reverseHttpEnabled = true;
  const networkName = sharedModels.NetworkName.EthereumSepolia;
  const network = sharedModels.NETWORK_CONFIG[networkName];
  const provider = createArtiEthersProvider();
  wallet.setFallbackProviderForNetwork(networkName, provider);
  wallet.setPollingProviderForNetwork(networkName, provider);
  const engine = getEngine();
  engine.quickSyncEvents = directQuickSyncEvents;
  await engine.loadNetwork(
    network.chain,
    network.proxyContract,
    network.relayAdaptContract,
    network.poseidonMerkleAccumulatorV3Contract,
    network.poseidonMerkleVerifierV3Contract,
    network.tokenVaultV3Contract,
    provider,
    provider,
    {
      [sharedModels.TXIDVersion.V2_PoseidonMerkle]: network.deploymentBlock ?? 0,
      [sharedModels.TXIDVersion.V3_PoseidonMerkle]:
        network.deploymentBlockPoseidonMerkleAccumulatorV3 ?? 0,
    },
    network.poi?.launchBlock,
    network.supportsV3,
  );
}

function denied(op) {
  return Promise.resolve()
    .then(op)
    .then(() => false)
    .catch(() => true);
}

async function nodeConnectDenied(host, port) {
  return denied(
    () =>
      new Promise((resolve, reject) => {
        const socket = net.connect(port, host);
        socket.once('connect', () => {
          socket.destroy();
          resolve();
        });
        socket.once('error', reject);
        socket.setTimeout(1000, () => {
          socket.destroy();
          reject(new Error('socket timeout'));
        });
      }),
  );
}

async function handle(method, params = {}) {
  switch (method) {
    case 'health':
      return {
        sdk_version: '10.8.6',
        shared_models_version: '8.0.1',
        node_compat:
          Object.keys(wallet).length > 0 && Object.keys(sharedModels).length > 0,
      };
    case 'load_wallet': {
      await ensureEngine();
      const mnemonic = params.mnemonic;
      const encryptionKey = params.encryption_key;
      if (typeof mnemonic !== 'string' || typeof encryptionKey !== 'string') {
        throw new Error('load_wallet requires mnemonic and encryption_key strings');
      }
      const creationBlockNumbers = params.creation_block_numbers ?? {
        [sharedModels.NetworkName.EthereumSepolia]: 0,
      };
      const info = await wallet.createRailgunWallet(
        encryptionKey,
        mnemonic,
        creationBlockNumbers,
      );
      return {
        wallet_id: info.id,
        shielded_address: info.railgunAddress,
      };
    }
    case 'load_network_arti': {
      await ensureNetworkLoadedThroughArti();
      return { loaded: true };
    }
    case 'refresh_balance': {
      await ensureNetworkLoadedThroughArti();
      const walletID = params.wallet_id;
      if (typeof walletID !== 'string') {
        throw new Error('refresh_balance requires wallet_id string');
      }
      const networkName = sharedModels.NetworkName.EthereumSepolia;
      const { chain } = sharedModels.NETWORK_CONFIG[networkName];
      await wallet.refreshBalances(chain, [walletID]);
      await wallet.refreshReceivePOIsForWallet(
        sharedModels.TXIDVersion.V2_PoseidonMerkle,
        networkName,
        walletID,
      );
      const railgunWallet = wallet.walletForID(walletID);
      const balance = await wallet.balanceForERC20Token(
        sharedModels.TXIDVersion.V2_PoseidonMerkle,
        railgunWallet,
        networkName,
        sharedModels.BaseTokenWrappedAddress[networkName],
        false,
      );
      const spendableBalance = await wallet.balanceForERC20Token(
        sharedModels.TXIDVersion.V2_PoseidonMerkle,
        railgunWallet,
        networkName,
        sharedModels.BaseTokenWrappedAddress[networkName],
        true,
      );
      return {
        token_address: sharedModels.BaseTokenWrappedAddress[networkName],
        balance: balance.toString(),
        spendable_balance: spendableBalance.toString(),
      };
    }
    case 'populate_unshield_base_token': {
      await ensureNetworkLoadedThroughArti();
      const walletID = params.wallet_id;
      const publicWalletAddress = params.public_wallet_address;
      const encryptionKey = params.encryption_key;
      if (typeof walletID !== 'string') {
        throw new Error('populate_unshield_base_token requires wallet_id string');
      }
      if (typeof publicWalletAddress !== 'string') {
        throw new Error(
          'populate_unshield_base_token requires public_wallet_address string',
        );
      }
      if (typeof encryptionKey !== 'string') {
        throw new Error('populate_unshield_base_token requires encryption_key string');
      }
      const amount = BigInt(params.amount_wei ?? 0);
      if (amount <= 0n) {
        throw new Error('populate_unshield_base_token requires positive amount_wei');
      }
      const networkName = sharedModels.NetworkName.EthereumSepolia;
      const tokenAddress = sharedModels.BaseTokenWrappedAddress[networkName];
      const { chain } = sharedModels.NETWORK_CONFIG[networkName];
      await wallet.refreshBalances(chain, [walletID]);
      await wallet.refreshReceivePOIsForWallet(
        sharedModels.TXIDVersion.V2_PoseidonMerkle,
        networkName,
        walletID,
      );
      const railgunWallet = wallet.walletForID(walletID);
      const spendableBalance = await wallet.balanceForERC20Token(
        sharedModels.TXIDVersion.V2_PoseidonMerkle,
        railgunWallet,
        networkName,
        tokenAddress,
        true,
      );
      if (spendableBalance < amount) {
        throw new Error(
          `insufficient spendable balance: have ${spendableBalance}, need ${amount}`,
        );
      }
      const wrappedERC20Amount = { tokenAddress, amount };
      const sendWithPublicWallet = true;
      const gasEstimateResponse =
        await wallet.gasEstimateForUnprovenUnshieldBaseToken(
          sharedModels.TXIDVersion.V2_PoseidonMerkle,
          networkName,
          publicWalletAddress,
          walletID,
          encryptionKey,
          wrappedERC20Amount,
          undefined,
          undefined,
          sendWithPublicWallet,
        );
      const provider = wallet.getFallbackProviderForNetwork(networkName);
      const feeData = await provider.getFeeData();
      const evmGasType = sharedModels.getEVMGasTypeForTransaction(
        networkName,
        sendWithPublicWallet,
      );
      const gasDetails = {
        evmGasType,
        gasEstimate: gasEstimateResponse.gasEstimate,
      };
      if (evmGasType === sharedModels.EVMGasType.Type2) {
        gasDetails.maxFeePerGas = feeData.maxFeePerGas ?? feeData.gasPrice;
        gasDetails.maxPriorityFeePerGas =
          feeData.maxPriorityFeePerGas ?? feeData.gasPrice;
      } else {
        gasDetails.gasPrice = feeData.gasPrice;
      }
      await wallet.generateUnshieldBaseTokenProof(
        sharedModels.TXIDVersion.V2_PoseidonMerkle,
        networkName,
        publicWalletAddress,
        walletID,
        encryptionKey,
        wrappedERC20Amount,
        undefined,
        sendWithPublicWallet,
        undefined,
        (progress) => {
          process.stderr.write(`unshield proof progress ${progress}\n`);
        },
      );
      const { transaction, nullifiers } =
        await wallet.populateProvedUnshieldBaseToken(
          sharedModels.TXIDVersion.V2_PoseidonMerkle,
          networkName,
          publicWalletAddress,
          walletID,
          wrappedERC20Amount,
          undefined,
          sendWithPublicWallet,
          undefined,
          gasDetails,
        );
      return {
        to: transaction.to,
        data: transaction.data,
        value: transaction.value ?? 0n,
        gas_limit: transaction.gasLimit,
        nullifiers,
        token_address: tokenAddress,
        amount,
      };
    }
    case 'populate_shield_base_token': {
      await ensureEngine();
      ensureNetworkContracts();
      const railgunAddress = params.railgun_address;
      const amountWei = params.amount_wei;
      const shieldPrivateKey =
        params.shield_private_key ??
        '0x0101010101010101010101010101010101010101010101010101010101010101';
      if (typeof railgunAddress !== 'string' || typeof amountWei !== 'string') {
        throw new Error(
          'populate_shield_base_token requires railgun_address and amount_wei strings',
        );
      }

      const networkName = sharedModels.NetworkName.EthereumSepolia;
      const wrappedERC20Amount = {
        tokenAddress: sharedModels.BaseTokenWrappedAddress[networkName],
        amount: BigInt(amountWei),
      };
      const gasDetails = {
        evmGasType: sharedModels.EVMGasType.Type2,
        gasEstimate: BigInt(params.gas ?? 0),
        maxFeePerGas: BigInt(params.max_fee_per_gas ?? 0),
        maxPriorityFeePerGas: BigInt(params.max_priority_fee_per_gas ?? 0),
      };
      const { transaction } = await wallet.populateShieldBaseToken(
        sharedModels.TXIDVersion.V2_PoseidonMerkle,
        networkName,
        railgunAddress,
        shieldPrivateKey,
        wrappedERC20Amount,
        params.gas ? gasDetails : undefined,
      );
      return {
        to: transaction.to,
        data: transaction.data,
        value: transaction.value?.toString() ?? '0',
      };
    }
    case 'sidecar-permissions-smoke': {
      const nodeNetHost = params.node_net_host ?? '127.0.0.1';
      const nodeNetPort = params.node_net_port ?? 53;
      return {
        fetch_denied: await denied(() => fetch('https://example.com')),
        connect_denied: await nodeConnectDenied('1.1.1.1', 53),
        node_net_denied: await nodeConnectDenied(nodeNetHost, nodeNetPort),
        write_denied: await denied(() =>
          fs.writeFile('/tmp/undercover-deny-write', 'x'),
        ),
        env_denied: !process.env.UNDERCOVER_FORBIDDEN_ENV,
        read_allowed: await denied(() => fs.readFile('/app/artifacts/manifest')).then(
          (isDenied) => !isDenied,
        ),
      };
    }
    default:
      throw new Error(`unknown method: ${method}`);
  }
}

const rl = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
});

rl.on('close', () => {
  process.exit(0);
});

rl.on('line', async (line) => {
  let id = null;
  try {
    const request = JSON.parse(line);
    if (request.undercover_reverse_rpc) {
      if (!handleReverseRpcResponse(request)) {
        throw new Error(`unknown reverse RPC response id: ${request.id}`);
      }
      return;
    }
    id = typeof request.id === 'number' ? request.id : null;
    if (request.jsonrpc !== '2.0' || typeof request.method !== 'string') {
      throw new Error('invalid JSON-RPC request');
    }
    const result = await handle(request.method, request.params);
    respond({ jsonrpc: '2.0', id, result });
  } catch (err) {
    respond({
      jsonrpc: '2.0',
      id,
      error: {
        code: -32603,
        message: err instanceof Error ? err.message : String(err),
        data: err instanceof Error ? err.stack : undefined,
      },
    });
  }
});
