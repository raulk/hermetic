use std::{path::Path, time::Duration};

use alloy_primitives::U256;
use anyhow::{anyhow, Result};
use serde::Deserialize;

use crate::{embedded::EmbeddedRailgun, rpc::TorRpcClient};

pub struct RailgunRuntime {
    inner: EmbeddedRailgun,
    rpc_client: Option<TorRpcClient>,
}

#[derive(Debug, Deserialize)]
pub struct Health {
    pub sdk_version: String,
    pub shared_models_version: String,
    pub node_compat: bool,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct PermissionSmoke {
    pub fetch_denied: bool,
    pub connect_denied: bool,
    pub node_net_denied: bool,
    pub write_denied: bool,
    pub env_denied: bool,
    pub read_allowed: bool,
}

#[derive(Debug, Deserialize)]
pub struct LoadedWallet {
    pub wallet_id: String,
    pub shielded_address: String,
}

#[derive(Debug, Deserialize)]
pub struct PopulatedTransaction {
    pub to: String,
    pub data: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshedBalance {
    pub token_address: String,
    pub balance: String,
    pub spendable_balance: String,
}

impl RailgunRuntime {
    /// Create a typed Railgun runtime facade.
    ///
    /// # Errors
    ///
    /// Returns an error when the embedded Deno runtime cannot be initialized.
    pub async fn new(workdir: &Path) -> Result<Self> {
        Ok(Self {
            inner: EmbeddedRailgun::new(workdir).await?,
            rpc_client: None,
        })
    }

    /// Attach reverse-RPC state used by SDK calls that need network data.
    #[must_use]
    pub fn with_rpc_client(mut self, rpc_client: TorRpcClient) -> Self {
        self.rpc_client = Some(rpc_client);
        self
    }

    /// Return SDK version and import health information.
    ///
    /// # Errors
    ///
    /// Returns an error when the embedded runtime call fails.
    pub async fn health(&mut self) -> Result<Health> {
        self.inner.call("health", serde_json::json!({})).await
    }

    /// Run the embedded runtime permission probe.
    ///
    /// # Errors
    ///
    /// Returns an error when the embedded runtime call fails.
    pub async fn permission_smoke(&mut self, node_net_port: u16) -> Result<PermissionSmoke> {
        self.inner
            .call(
                "runtime-permissions-smoke",
                serde_json::json!({ "node_net_port": node_net_port }),
            )
            .await
    }

    /// Load a Railgun wallet into the embedded runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the SDK rejects the wallet parameters or the
    /// embedded runtime call fails.
    pub async fn load_wallet(
        &mut self,
        mnemonic: &str,
        encryption_key: &str,
        creation_block: Option<u64>,
    ) -> Result<LoadedWallet> {
        let params = match creation_block {
            Some(creation_block) => serde_json::json!({
                "mnemonic": mnemonic,
                "encryption_key": encryption_key,
                "creation_block_numbers": {
                    "Ethereum_Sepolia": creation_block,
                },
            }),
            None => serde_json::json!({
                "mnemonic": mnemonic,
                "encryption_key": encryption_key,
            }),
        };
        self.inner.call("load_wallet", params).await
    }

    /// Populate a base-token shield transaction.
    ///
    /// # Errors
    ///
    /// Returns an error when the SDK cannot produce calldata.
    pub async fn populate_shield_base_token(
        &mut self,
        railgun_address: &str,
        amount_wei: &U256,
    ) -> Result<PopulatedTransaction> {
        self.inner
            .call(
                "populate_shield_base_token",
                serde_json::json!({
                    "railgun_address": railgun_address,
                    "amount_wei": amount_wei.to_string(),
                }),
            )
            .await
    }

    /// Refresh private balance state through Rust-owned Tor egress.
    ///
    /// # Errors
    ///
    /// Returns an error when quick-sync, RPC, or balance decryption fails.
    pub async fn refresh_balance(&mut self, wallet_id: &str) -> Result<RefreshedBalance> {
        let rpc_client = self.rpc_client()?;
        tokio::time::timeout(
            Duration::from_mins(3),
            self.inner.call_with_reverse_rpc(
                "refresh_balance",
                serde_json::json!({ "wallet_id": wallet_id }),
                rpc_client.clone(),
            ),
        )
        .await?
    }

    /// Prove and prepare a base-token unshield transaction.
    ///
    /// # Errors
    ///
    /// Returns an error when proving, quick-sync, or RPC access fails.
    pub async fn prepare_unshield_base_token(
        &mut self,
        wallet_id: &str,
        recipient: &str,
        encryption_key: &str,
        amount_wei: &U256,
    ) -> Result<PopulatedTransaction> {
        let rpc_client = self.rpc_client()?;
        tokio::time::timeout(
            Duration::from_mins(15),
            self.inner.call_with_reverse_rpc(
                "prepare_unshield_base_token",
                serde_json::json!({
                    "wallet_id": wallet_id,
                    "public_wallet_address": recipient,
                    "encryption_key": encryption_key,
                    "amount_wei": amount_wei.to_string(),
                }),
                rpc_client.clone(),
            ),
        )
        .await?
    }

    fn rpc_client(&self) -> Result<&TorRpcClient> {
        self.rpc_client
            .as_ref()
            .ok_or_else(|| anyhow!("Railgun runtime was created without a Tor RPC client"))
    }
}
