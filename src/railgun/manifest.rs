use std::path::Path;

use anyhow::{anyhow, Context as _, Result};
use serde::{Deserialize, Serialize};

const MANIFEST_VERSION: u8 = 1;
const MANIFEST_PATH: &str = "artifacts/wallets.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletRecord {
    pub label: String,
    pub wallet_id: String,
    pub shielded_address: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WalletManifest {
    pub version: u8,
    pub wallets: Vec<WalletRecord>,
}

impl Default for WalletManifest {
    fn default() -> Self {
        Self {
            version: MANIFEST_VERSION,
            wallets: Vec::new(),
        }
    }
}

impl WalletManifest {
    /// Load the wallet manifest from the artifact directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the manifest exists but cannot be read or parsed.
    pub fn load(workdir: &Path) -> Result<Self> {
        let path = workdir.join(MANIFEST_PATH);
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let manifest: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", path.display()))?;
        anyhow::ensure!(
            manifest.version == MANIFEST_VERSION,
            "unsupported wallet manifest version {}",
            manifest.version
        );
        Ok(manifest)
    }

    /// Persist the wallet manifest under the artifact directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the manifest cannot be serialized or written.
    pub fn save(&self, workdir: &Path) -> Result<()> {
        let path = workdir.join(MANIFEST_PATH);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(self).context("encoding wallet manifest")?;
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))
    }

    /// Insert or replace a wallet by label.
    pub fn upsert(&mut self, record: WalletRecord) {
        if let Some(existing) = self
            .wallets
            .iter_mut()
            .find(|wallet| wallet.label == record.label)
        {
            *existing = record;
        } else {
            self.wallets.push(record);
        }
    }

    /// Find a wallet by label or wallet ID.
    ///
    /// # Errors
    ///
    /// Returns an error when no wallet matches the selector.
    pub fn select(&self, selector: &str) -> Result<&WalletRecord> {
        self.wallets
            .iter()
            .find(|wallet| wallet.label == selector || wallet.wallet_id == selector)
            .ok_or_else(|| anyhow!("wallet not found in manifest: {selector}"))
    }
}

/// Validate a user-facing wallet label.
///
/// # Errors
///
/// Returns an error for empty labels.
pub fn validate_label(label: &str) -> Result<()> {
    anyhow::ensure!(!label.trim().is_empty(), "wallet label cannot be empty");
    Ok(())
}
