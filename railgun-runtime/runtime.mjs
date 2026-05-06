import * as wallet from "@railgun-community/wallet";
import * as sharedModels from "@railgun-community/shared-models";
import { BrowserProvider, Network, Wallet as EthersWallet } from "ethers";
import * as snarkjs from "snarkjs";
import packageJson from "./package.json";
import { Buffer } from "node:buffer";
import process from "node:process";
import { DatabaseSync } from "node:sqlite";

const {
  op_hermetic_artifact_exists,
  op_hermetic_log,
  op_hermetic_progress,
  op_hermetic_read_artifact,
  op_hermetic_service_endpoint,
  op_hermetic_reverse_request,
  op_hermetic_write_artifact,
} = globalThis.__hermetic_ops;

const workdir = globalThis.__hermetic_workdir;
const DENIED_FETCH_PROBE_URL = "https://example.com";
const { AbstractLevelDOWN } = require("abstract-leveldown");
const AbstractIterator = require("abstract-leveldown/abstract-iterator");

globalThis.__dirname = `${workdir}/embedded`;
globalThis.__filename = `${globalThis.__dirname}/railgun_runtime.bundle.mjs`;
globalThis.__hermetic_deno_fetch = globalThis.fetch;
Object.defineProperty(process, "env", {
  value: {},
  configurable: true,
  enumerable: true,
  writable: true,
});

let engineStarted = false;
let networkLoaded = false;
let localFilterID = 1;
const localFilters = new Map();
const serviceEndpoints = {
  graphql: new URL(op_hermetic_service_endpoint("graphql")),
  poi: new URL(op_hermetic_service_endpoint("poi")),
};
const poiNodeURLs = [serviceEndpoints.poi.toString()];
const NETWORK_NAME = sharedModels.NetworkName.EthereumSepolia;
const TXID_VERSION = sharedModels.TXIDVersion.V2_PoseidonMerkle;
const BASE_TOKEN_ADDRESS = sharedModels.BaseTokenWrappedAddress[NETWORK_NAME];
let snarkMutationDepth = 0;

function trace(message) {
  op_hermetic_log(`[hermetic-runtime] ${message}`);
}

function describeItem(item) {
  if (item == null) {
    return "null";
  }
  if (typeof item === "string") {
    return `string:${item.length}`;
  }
  if (item.byteLength != null) {
    return `bytes:${item.byteLength}`;
  }
  return typeof item;
}

function isJsonArtifact(relativePath) {
  return relativePath.toLowerCase().endsWith(".json");
}

function artifactBytes(item) {
  if (typeof item === "string") {
    return new TextEncoder().encode(item);
  }
  return item;
}

function randomHexPrivateKey() {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return `0x${
    Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")
  }`;
}

const artifactStore = new wallet.ArtifactStore(
  (relativePath) => {
    const started = Date.now();
    const item = op_hermetic_artifact_exists(relativePath)
      ? op_hermetic_read_artifact(relativePath)
      : null;
    const artifact = item != null && isJsonArtifact(relativePath)
      ? new TextDecoder().decode(item)
      : item;
    trace(
      `artifact read path=${relativePath} result=${describeItem(artifact)} ms=${
        Date.now() - started
      }`,
    );
    return artifact;
  },
  (dir, relativePath, item) => {
    trace(`artifact write path=${relativePath} item=${describeItem(item)}`);
    return op_hermetic_write_artifact(dir, relativePath, artifactBytes(item));
  },
  (relativePath) => {
    const exists = op_hermetic_artifact_exists(relativePath);
    trace(`artifact exists path=${relativePath} result=${exists}`);
    return exists;
  },
);

const DB_PATH = `${workdir}/artifacts/railgun.db`;

class SqliteLevelDOWN extends AbstractLevelDOWN {
  constructor() {
    super({
      bufferKeys: false,
      promises: false,
      snapshots: false,
      permanence: true,
      clear: true,
      createIfMissing: true,
      errorIfExists: false,
      seek: false,
      streams: true,
      encodings: {
        buffer: true,
        utf8: true,
        json: true,
      },
    });
    this.db = null;
    this.statements = null;
  }

  _open(_options, callback) {
    try {
      Deno.mkdirSync(`${workdir}/artifacts`, { recursive: true });
      this.db = new DatabaseSync(DB_PATH);
      this.db.exec(`
        CREATE TABLE IF NOT EXISTS kv (
          key TEXT PRIMARY KEY,
          value BLOB NOT NULL
        ) STRICT
      `);
      this.statements = {
        get: this.db.prepare("SELECT value FROM kv WHERE key = ?"),
        put: this.db.prepare(
          "INSERT INTO kv(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        ),
        del: this.db.prepare("DELETE FROM kv WHERE key = ?"),
      };
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _close(callback) {
    try {
      this.statements = null;
      this.db?.close();
      this.db = null;
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _get(key, options, callback) {
    try {
      const row = this.statement("get").get(String(key));
      if (row == null) {
        callback(notFound(key));
        return;
      }
      callback(null, decodeDatabaseValue(row.value, options));
    } catch (error) {
      callback(error);
    }
  }

  _put(key, value, _options, callback) {
    try {
      this.statement("put").run(String(key), Buffer.from(value));
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _del(key, _options, callback) {
    try {
      this.statement("del").run(String(key));
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _batch(ops, _options, callback) {
    try {
      this.db.exec("BEGIN IMMEDIATE");
      try {
        for (const op of ops) {
          if (op.type === "put") {
            this.statement("put").run(String(op.key), Buffer.from(op.value));
          } else if (op.type === "del") {
            this.statement("del").run(String(op.key));
          }
        }
        this.db.exec("COMMIT");
      } catch (error) {
        this.db.exec("ROLLBACK");
        throw error;
      }
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _clear(options, callback) {
    try {
      const { where, params } = rangeWhere(options);
      this.db.prepare(`DELETE FROM kv${where}`).run(...params);
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _iterator(options) {
    return new SqliteIterator(this, options);
  }

  statement(name) {
    if (this.statements == null) {
      throw new Error("Railgun SQLite database is not open");
    }
    return this.statements[name];
  }

  all(options) {
    const { where, params } = rangeWhere(options);
    const order = options.reverse ? "DESC" : "ASC";
    const limit = typeof options.limit === "number" && options.limit >= 0
      ? ` LIMIT ${options.limit}`
      : "";
    return this.db.prepare(
      `SELECT key, value FROM kv${where} ORDER BY key ${order}${limit}`,
    ).all(...params);
  }
}

class SqliteIterator extends AbstractIterator {
  constructor(db, options) {
    super(db);
    this.options = options;
    this.entries = db.all(options);
    this.index = 0;
  }

  _next(callback) {
    try {
      if (this.index >= this.entries.length) {
        callback();
        return;
      }
      const row = this.entries[this.index++];
      callback(
        null,
        this.options.keys
          ? encodeIteratorKey(row.key, this.options)
          : undefined,
        this.options.values
          ? decodeDatabaseValue(row.value, this.options)
          : undefined,
      );
    } catch (error) {
      callback(error);
    }
  }

  _end(callback) {
    this.entries = [];
    callback();
  }
}

function decodeDatabaseValue(value, options) {
  const bytes = Buffer.from(value);
  return options.asBuffer === false ? bytes.toString("utf8") : bytes;
}

function encodeIteratorKey(key, options) {
  return options.keyAsBuffer === false ? key : Buffer.from(key);
}

function rangeWhere(options) {
  const clauses = [];
  const params = [];
  for (
    const [operator, sql] of [
      ["gt", "key > ?"],
      ["gte", "key >= ?"],
      ["lt", "key < ?"],
      ["lte", "key <= ?"],
    ]
  ) {
    if (options[operator] != null) {
      clauses.push(sql);
      params.push(String(options[operator]));
    }
  }
  return {
    where: clauses.length === 0 ? "" : ` WHERE ${clauses.join(" AND ")}`,
    params,
  };
}

function notFound(key) {
  const error = new Error(`Key not found in database [${String(key)}]`);
  error.notFound = true;
  error.status = 404;
  error.code = "LEVEL_NOT_FOUND";
  return error;
}

async function withSnarkSingleThread(label, operation) {
  const started = Date.now();
  if (snarkMutationDepth !== 0) {
    throw new Error(`nested snark operation is not supported: ${label}`);
  }
  snarkMutationDepth += 1;
  const previousBrowser = process.browser;
  const hadWorker = Object.hasOwn(globalThis, "Worker");
  const previousWorker = globalThis.Worker;
  trace(`snark ${label} start`);
  try {
    process.browser = true;
    globalThis.Worker = undefined;
    const result = await operation();
    trace(`snark ${label} ok ms=${Date.now() - started}`);
    return result;
  } catch (error) {
    trace(`snark ${label} failed ms=${Date.now() - started}`);
    trace(`snark ${label} error ${String(error?.stack ?? error)}`);
    throw error;
  } finally {
    snarkMutationDepth -= 1;
    process.browser = previousBrowser;
    if (hadWorker) {
      globalThis.Worker = previousWorker;
    } else {
      delete globalThis.Worker;
    }
  }
}

const groth16 = {
  ...snarkjs.groth16,
  fullProve(input, wasm, zkey, logger, wtnsCalcOptions, proverOptions) {
    return withSnarkSingleThread("fullProve", () =>
      snarkjs.groth16.fullProve(
        input,
        wasm,
        zkey,
        logger,
        { ...(wtnsCalcOptions ?? {}), singleThread: true },
        { ...(proverOptions ?? {}), singleThread: true },
      ));
  },
  verify(...args) {
    return withSnarkSingleThread(
      "verify",
      () => snarkjs.groth16.verify(...args),
    );
  },
};

function endpointService(url) {
  for (const [service, endpoint] of Object.entries(serviceEndpoints)) {
    if (url.origin === endpoint.origin) {
      return service;
    }
  }
  throw new Error(`Railgun SDK requested non-hosted URL: ${url.origin}`);
}

async function hostServiceFetch(input, init = {}) {
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

globalThis.fetch = hostServiceFetch;
require("@whatwg-node/fetch").fetch = hostServiceFetch;

async function ensureEngine() {
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

async function denied(op) {
  try {
    await op();
    return false;
  } catch (_) {
    return true;
  }
}

async function permissionSmoke(params = {}) {
  const net = require("node:net");
  const nodeNetHost = params.node_net_host ?? "127.0.0.1";
  const nodeNetPort = params.node_net_port ?? 53;
  return {
    fetch_denied: await denied(() =>
      globalThis.__hermetic_deno_fetch(DENIED_FETCH_PROBE_URL)
    ),
    connect_denied: await denied(() =>
      Deno.connect({ hostname: "1.1.1.1", port: 53 })
    ),
    node_net_denied: await denied(
      () =>
        new Promise((resolve, reject) => {
          const socket = net.connect(nodeNetPort, nodeNetHost);
          socket.once("connect", () => {
            socket.destroy();
            resolve();
          });
          socket.once("error", reject);
          socket.setTimeout(1000, () => {
            socket.destroy();
            reject(new Error("socket timeout"));
          });
        }),
    ),
    write_denied: await denied(() =>
      Deno.writeTextFile("/tmp/hermetic-deny-write", "x")
    ),
    env_denied: await denied(() => Deno.env.get("HERMETIC_FORBIDDEN_ENV")),
    read_allowed:
      !(await denied(() => Deno.readTextFile(`${workdir}/artifacts/manifest`))),
  };
}

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

async function ensureNetworkLoadedThroughTor() {
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
      .updateTreesFromWriteQueue(
        true,
      );
  }
  await engine.syncRailgunTransactionsV2(chain, "hermetic balance refresh");
}

async function refreshSdkBalanceState(
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

async function createWalletFromMnemonic(params) {
  await ensureEngine();
  const mnemonic = params.mnemonic;
  const encryptionKey = params.encryption_key;
  if (typeof mnemonic !== "string" || typeof encryptionKey !== "string") {
    throw new Error("load_wallet requires mnemonic and encryption_key strings");
  }
  const info = await wallet.createRailgunWallet(
    encryptionKey,
    mnemonic,
    undefined,
  );
  return {
    wallet_id: info.id,
    shielded_address: info.railgunAddress,
  };
}

export async function handle(method, params = {}) {
  switch (method) {
    case "health":
      return {
        sdk_version: packageJson.dependencies["@railgun-community/wallet"],
        shared_models_version:
          packageJson.dependencies["@railgun-community/shared-models"],
        node_compat: typeof wallet.startRailgunEngine === "function" &&
          typeof wallet.createRailgunWallet === "function" &&
          typeof wallet.loadWalletByID === "function" &&
          typeof wallet.refreshBalances === "function" &&
          typeof wallet.setFallbackProviderForNetwork === "function" &&
          sharedModels.NETWORK_CONFIG != null,
      };
    case "load_wallet":
      return createWalletFromMnemonic(params);
    case "create_wallet": {
      await ensureEngine();
      const encryptionKey = params.encryption_key;
      if (typeof encryptionKey !== "string") {
        throw new Error("create_wallet requires encryption_key string");
      }
      const mnemonic = EthersWallet.createRandom().mnemonic?.phrase;
      if (typeof mnemonic !== "string") {
        throw new Error("ethers failed to generate a mnemonic");
      }
      const walletInfo = await createWalletFromMnemonic({
        ...params,
        mnemonic,
      });
      return { ...walletInfo, mnemonic };
    }
    case "load_wallet_by_id": {
      await ensureEngine();
      const walletID = params.wallet_id;
      const encryptionKey = params.encryption_key;
      if (typeof walletID !== "string" || typeof encryptionKey !== "string") {
        throw new Error(
          "load_wallet_by_id requires wallet_id and encryption_key strings",
        );
      }
      const info = await wallet.loadWalletByID(encryptionKey, walletID, false);
      return {
        wallet_id: info.id,
        shielded_address: info.railgunAddress,
      };
    }
    case "refresh_balance": {
      await ensureNetworkLoadedThroughTor();
      const walletID = params.wallet_id;
      if (typeof walletID !== "string") {
        throw new Error("refresh_balance requires wallet_id string");
      }
      const { chain } = sharedModels.NETWORK_CONFIG[NETWORK_NAME];
      await refreshSdkBalanceState(chain, walletID);
      const railgunWallet = wallet.walletForID(walletID);
      const balance = await wallet.balanceForERC20Token(
        TXID_VERSION,
        railgunWallet,
        NETWORK_NAME,
        BASE_TOKEN_ADDRESS,
        false,
      );
      const spendableBalance = await wallet.balanceForERC20Token(
        TXID_VERSION,
        railgunWallet,
        NETWORK_NAME,
        BASE_TOKEN_ADDRESS,
        true,
      );
      return {
        token_address: BASE_TOKEN_ADDRESS,
        balance: balance.toString(),
        spendable_balance: spendableBalance.toString(),
      };
    }
    case "prepare_unshield_base_token": {
      await ensureNetworkLoadedThroughTor();
      const walletID = params.wallet_id;
      const publicWalletAddress = params.public_wallet_address;
      const encryptionKey = params.encryption_key;
      if (typeof walletID !== "string") {
        throw new Error(
          "prepare_unshield_base_token requires wallet_id string",
        );
      }
      if (typeof publicWalletAddress !== "string") {
        throw new Error(
          "prepare_unshield_base_token requires public_wallet_address string",
        );
      }
      if (typeof encryptionKey !== "string") {
        throw new Error(
          "prepare_unshield_base_token requires encryption_key string",
        );
      }
      const amount = BigInt(params.amount_wei ?? 0);
      if (amount <= 0n) {
        throw new Error(
          "prepare_unshield_base_token requires positive amount_wei",
        );
      }
      const { chain } = sharedModels.NETWORK_CONFIG[NETWORK_NAME];
      await refreshSdkBalanceState(chain, walletID, {
        requirePoiRefresh: true,
      });
      const railgunWallet = wallet.walletForID(walletID);
      const spendableBalance = await wallet.balanceForERC20Token(
        TXID_VERSION,
        railgunWallet,
        NETWORK_NAME,
        BASE_TOKEN_ADDRESS,
        true,
      );
      if (spendableBalance < amount) {
        throw new Error(
          `insufficient spendable balance: have ${spendableBalance}, need ${amount}`,
        );
      }
      const wrappedERC20Amount = { tokenAddress: BASE_TOKEN_ADDRESS, amount };
      const sendWithPublicWallet = true;
      const gasEstimateResponse = await wallet
        .gasEstimateForUnprovenUnshieldBaseToken(
          TXID_VERSION,
          NETWORK_NAME,
          publicWalletAddress,
          walletID,
          encryptionKey,
          wrappedERC20Amount,
          undefined,
          undefined,
          sendWithPublicWallet,
        );
      const provider = wallet.getFallbackProviderForNetwork(NETWORK_NAME);
      const feeData = await provider.getFeeData();
      const evmGasType = sharedModels.getEVMGasTypeForTransaction(
        NETWORK_NAME,
        sendWithPublicWallet,
      );
      const gasDetails = {
        evmGasType,
        gasEstimate: gasEstimateResponse.gasEstimate,
      };
      if (evmGasType === sharedModels.EVMGasType.Type2) {
        gasDetails.maxFeePerGas = feeData.maxFeePerGas ?? feeData.gasPrice;
        gasDetails.maxPriorityFeePerGas = feeData.maxPriorityFeePerGas ??
          feeData.gasPrice;
      } else {
        gasDetails.gasPrice = feeData.gasPrice;
      }
      await wallet.generateUnshieldBaseTokenProof(
        TXID_VERSION,
        NETWORK_NAME,
        publicWalletAddress,
        walletID,
        encryptionKey,
        wrappedERC20Amount,
        undefined,
        sendWithPublicWallet,
        undefined,
        (progress) => {
          op_hermetic_progress(`unshield proof progress ${progress}`);
        },
      );
      const { transaction, nullifiers } = await wallet
        .populateProvedUnshieldBaseToken(
          TXID_VERSION,
          NETWORK_NAME,
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
        gas_limit: transaction.gasLimit?.toString(),
        nullifiers,
        token_address: tokenAddress,
        amount,
      };
    }
    case "populate_shield_base_token": {
      await ensureNetworkLoadedThroughTor();
      const railgunAddress = params.railgun_address;
      const amountWei = params.amount_wei;
      const shieldPrivateKey = randomHexPrivateKey();
      if (typeof railgunAddress !== "string" || typeof amountWei !== "string") {
        throw new Error(
          "populate_shield_base_token requires railgun_address and amount_wei strings",
        );
      }

      const wrappedERC20Amount = {
        tokenAddress: BASE_TOKEN_ADDRESS,
        amount: BigInt(amountWei),
      };
      const { transaction } = await wallet.populateShieldBaseToken(
        TXID_VERSION,
        NETWORK_NAME,
        railgunAddress,
        shieldPrivateKey,
        wrappedERC20Amount,
      );
      return {
        to: transaction.to,
        data: transaction.data,
        value: transaction.value?.toString() ?? "0",
      };
    }
    case "runtime-permissions-smoke":
      return permissionSmoke(params);
    default:
      throw new Error(`unknown method: ${method}`);
  }
}

export async function invoke(method, params = {}) {
  try {
    return stringify({
      ok: true,
      result: await handle(method, params),
    });
  } catch (error) {
    return stringify({
      ok: false,
      error: String(error?.stack ?? error),
    });
  }
}

function stringify(value) {
  return JSON.stringify(
    value,
    (_, item) => typeof item === "bigint" ? item.toString() : item,
  );
}
