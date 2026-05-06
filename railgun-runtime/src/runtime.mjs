// Entry point for the embedded Railgun runtime bundle. Owns the JS-side
// dispatch surface and the wallet-level helpers; delegates storage,
// network, snark, and permission concerns to its siblings.

import process from "node:process";

import * as wallet from "@railgun-community/wallet";
import * as sharedModels from "@railgun-community/shared-models";
import { Wallet as EthersWallet } from "ethers";

import packageJson from "../package.json";

// Shadow process.env with an empty object so SDK env reads return
// undefined silently. The Railgun SDK's bundled logger reads
// `process.env.DEBUG` from inside `handle()` (not at module init), and
// Deno would otherwise throw NotCapable since DEBUG is not in the env
// allowlist. This runs before the first handle() invocation, which is
// what matters; static imports above only set up symbols.
process.env = {};

import { randomHexPrivateKey } from "./artifacts.mjs";
import { op_hermetic_progress } from "./host-ops.mjs";
import {
  BASE_TOKEN_ADDRESS,
  ensureEngine,
  ensureNetworkLoadedThroughTor,
  NETWORK_NAME,
  refreshSdkBalanceState,
  TXID_VERSION,
} from "./network.mjs";
import { permissionSmoke } from "./permissions.mjs";

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
        token_address: BASE_TOKEN_ADDRESS,
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
