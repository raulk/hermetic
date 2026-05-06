use std::path::{Path, PathBuf};
use std::time::Duration;

use alloy_primitives::U256;
use anyhow::{anyhow, Context as _, Result};
use deno_runtime::deno_core::resolve_url;
use deno_runtime::deno_permissions::PermissionsOptions;
use serde::Deserialize;

use crate::embedded::{permissions_from_options, EmbeddedDeno, EmbeddedHostState};
use crate::rpc::TorRpcClient;

pub mod manifest;
pub mod reverse;

pub struct RailgunRuntime {
    inner: EmbeddedDeno,
    rpc_client: Option<TorRpcClient>,
}

#[derive(Debug)]
pub struct Artifact {
    workdir: PathBuf,
    root: PathBuf,
}

impl Artifact {
    #[must_use]
    pub fn new(workdir: &Path) -> Self {
        Self {
            workdir: workdir.to_path_buf(),
            root: workdir.join("artifacts"),
        }
    }

    #[must_use]
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Read a Railgun artifact as raw bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the artifact exists but cannot be read.
    pub fn read(&self, relative_path: &str) -> Result<Vec<u8>> {
        std::fs::read(self.root.join(relative_path)).map_err(Into::into)
    }

    /// Write a Railgun artifact under the artifact root.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination directory cannot be created or the
    /// artifact cannot be written.
    pub fn write(&self, dir: &str, relative_path: &str, bytes: &[u8]) -> Result<()> {
        std::fs::create_dir_all(self.root.join(dir))?;
        std::fs::write(self.root.join(relative_path), bytes)?;
        Ok(())
    }

    #[must_use]
    pub fn exists(&self, relative_path: &str) -> bool {
        self.root.join(relative_path).exists()
    }
}

#[derive(Debug, Deserialize)]
pub struct Health {
    pub sdk_version: String,
    pub shared_models_version: String,
    pub node_compat: bool,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct Permissions {
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
pub struct CreatedWallet {
    pub wallet_id: String,
    pub shielded_address: String,
    pub mnemonic: String,
}

#[derive(Debug, Deserialize)]
pub struct PopulatedTransaction {
    pub to: String,
    pub data: String,
    pub value: String,
    pub gas_limit: Option<String>,
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
        let workdir = std::fs::canonicalize(workdir).context("resolving Railgun workdir")?;
        let bundle_path = workdir.join("embedded/railgun_runtime.bundle.mjs");
        let bundle = std::fs::read_to_string(&bundle_path)
            .with_context(|| format!("reading {}", bundle_path.display()))?;
        let main_module = resolve_url("file:///hermetic-embedded-railgun.mjs")?;
        let permissions = railgun_permissions(&workdir)?;
        let host_state = EmbeddedHostState::new(Artifact::new(&workdir));
        Ok(Self {
            inner: EmbeddedDeno::load_esm(&main_module, bundle, permissions, host_state).await?,
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
    pub async fn check_perms(&mut self, node_net_port: u16) -> Result<Permissions> {
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
    ) -> Result<LoadedWallet> {
        self.inner
            .call(
                "load_wallet",
                serde_json::json!({
                    "mnemonic": mnemonic,
                    "encryption_key": encryption_key,
                }),
            )
            .await
    }

    /// Create a new Railgun wallet and return its generated mnemonic once.
    ///
    /// # Errors
    ///
    /// Returns an error when the SDK cannot create or persist the wallet.
    pub async fn create_wallet(&mut self, encryption_key: &str) -> Result<CreatedWallet> {
        self.inner
            .call(
                "create_wallet",
                serde_json::json!({
                    "encryption_key": encryption_key,
                }),
            )
            .await
    }

    /// Load an SDK-managed wallet by ID.
    ///
    /// # Errors
    ///
    /// Returns an error when the wallet cannot be decrypted or loaded.
    pub async fn load_wallet_by_id(
        &mut self,
        wallet_id: &str,
        encryption_key: &str,
    ) -> Result<LoadedWallet> {
        self.inner
            .call(
                "load_wallet_by_id",
                serde_json::json!({
                    "wallet_id": wallet_id,
                    "encryption_key": encryption_key,
                }),
            )
            .await
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
        let rpc_client = self.rpc_client()?;
        self.inner
            .call_with_reverse_rpc(
                "populate_shield_base_token",
                serde_json::json!({
                    "railgun_address": railgun_address,
                    "amount_wei": amount_wei.to_string(),
                }),
                rpc_client.clone(),
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

fn railgun_permissions(
    workdir: &Path,
) -> Result<deno_runtime::deno_permissions::PermissionsContainer> {
    let artifacts = workdir.join("artifacts").to_string_lossy().to_string();
    let embedded = workdir.join("embedded").to_string_lossy().to_string();
    let wasm_packages = workdir
        .join("railgun-runtime/node_modules/@railgun-community")
        .to_string_lossy()
        .to_string();
    permissions_from_options(&PermissionsOptions {
        allow_read: Some(vec![artifacts.clone(), embedded, wasm_packages]),
        allow_write: Some(vec![artifacts]),
        allow_env: Some(vec![
            "WS_NO_BUFFER_UTIL".to_string(),
            "WS_NO_UTF_8_VALIDATE".to_string(),
            "READABLE_STREAM".to_string(),
            "NODE_ENV".to_string(),
        ]),
        allow_sys: Some(vec!["cpus".to_string()]),
        prompt: false,
        ..Default::default()
    })
}
