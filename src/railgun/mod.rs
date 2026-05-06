//! Typed Rust facade over the embedded Railgun SDK runtime. Modeled as a
//! typestate so the compiler enforces the rule "methods needing reverse
//! RPC require a connected runtime."

use std::marker::PhantomData;
use std::path::Path;
use std::time::Duration;

use alloy_primitives::U256;
use anyhow::{Context as _, Result};
use deno_runtime::deno_core::resolve_url;
use deno_runtime::deno_permissions::{PermissionsContainer, PermissionsOptions};

use crate::embedded::{permissions_from_options, EmbeddedDeno, EmbeddedHostState};

pub mod artifacts;
pub mod manifest;
pub mod reverse;
pub mod types;

pub use artifacts::Artifact;
pub use reverse::ReverseRpcService;
pub use types::{
    CreatedWallet, Health, LoadedWallet, PermissionsReport, PopulatedTransaction, RefreshedBalance,
};

/// Marker: runtime constructed but no reverse-RPC service attached. Wallet
/// IO, health, and permission probes are available; methods that need
/// network data are not.
pub struct Disconnected;

/// Marker: runtime has a reverse-RPC service attached and may make calls
/// that route SDK-emitted JSON-RPC and HTTP through Tor.
pub struct Connected;

pub struct Runtime<S> {
    inner: EmbeddedDeno,
    _state: PhantomData<S>,
}

pub type RailgunRuntime = Runtime<Disconnected>;
pub type ConnectedRuntime = Runtime<Connected>;

impl<S> Runtime<S> {
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
    pub async fn check_perms(&mut self, node_net_port: u16) -> Result<PermissionsReport> {
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
}

impl Runtime<Disconnected> {
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
            _state: PhantomData,
        })
    }

    /// Attach a reverse-RPC service for the duration of the runtime.
    ///
    /// The Connected runtime can issue SDK calls that route JSON-RPC and
    /// reverse HTTP through the supplied service.
    #[must_use]
    pub fn connect(mut self, reverse: ReverseRpcService) -> ConnectedRuntime {
        self.inner.set_reverse(Some(reverse));
        Runtime {
            inner: self.inner,
            _state: PhantomData,
        }
    }
}

impl Runtime<Connected> {
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
        tokio::time::timeout(
            Duration::from_mins(3),
            self.inner.call(
                "refresh_balance",
                serde_json::json!({ "wallet_id": wallet_id }),
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
        tokio::time::timeout(
            Duration::from_mins(15),
            self.inner.call(
                "prepare_unshield_base_token",
                serde_json::json!({
                    "wallet_id": wallet_id,
                    "public_wallet_address": recipient,
                    "encryption_key": encryption_key,
                    "amount_wei": amount_wei.to_string(),
                }),
            ),
        )
        .await?
    }
}

fn railgun_permissions(workdir: &Path) -> Result<PermissionsContainer> {
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
