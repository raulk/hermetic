use std::path::Path;

use anyhow::{anyhow, Context as _, Result};
use serde::{Deserialize, Serialize};

const MANIFEST_PATH: &str = "artifacts/wallets.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletRecord {
    pub label: String,
    pub wallet_id: String,
    pub shielded_address: String,
}

impl From<(String, crate::railgun::LoadedWallet)> for WalletRecord {
    fn from((label, wallet): (String, crate::railgun::LoadedWallet)) -> Self {
        Self {
            label,
            wallet_id: wallet.wallet_id,
            shielded_address: wallet.shielded_address,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WalletManifest {
    pub wallets: Vec<WalletRecord>,
}

impl WalletManifest {
    /// Load the wallet manifest from the artifact directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the manifest exists but cannot be read or parsed.
    pub fn load(workdir: &Path) -> Result<Self> {
        let path = workdir.join(MANIFEST_PATH);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
        };
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
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

    /// Load the manifest, upsert a wallet record, and save back.
    ///
    /// # Errors
    ///
    /// Returns an error when load or save fails.
    pub fn upsert_record(workdir: &Path, record: WalletRecord) -> Result<()> {
        let mut manifest = Self::load(workdir)?;
        manifest.upsert(record);
        manifest.save(workdir)
    }
}

/// Validate a user-facing wallet label for use as a clap `value_parser`.
///
/// # Errors
///
/// Returns an error string for empty or whitespace-only labels.
pub fn validate_label(label: &str) -> Result<String, String> {
    if label.trim().is_empty() {
        return Err("wallet label cannot be empty".to_owned());
    }
    Ok(label.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{validate_label, WalletManifest, WalletRecord};

    fn record(label: &str, wallet_id: &str) -> WalletRecord {
        WalletRecord {
            label: label.into(),
            wallet_id: wallet_id.into(),
            shielded_address: "0zk1qy0000000000000000000000000000000000000000000000000000000"
                .into(),
        }
    }

    // ── upsert ───────────────────────────────────────────────────────────────

    #[test]
    fn upsert_replaces_existing_label() {
        let mut manifest = WalletManifest::default();
        manifest.upsert(record("main", "id-first"));
        manifest.upsert(record("main", "id-second"));
        assert_eq!(
            manifest.wallets.len(),
            1,
            "duplicate label must not grow the list"
        );
        assert_eq!(manifest.wallets[0].wallet_id, "id-second");
    }

    #[test]
    fn upsert_appends_new_label() {
        let mut manifest = WalletManifest::default();
        manifest.upsert(record("alice", "id-alice"));
        manifest.upsert(record("bob", "id-bob"));
        assert_eq!(manifest.wallets.len(), 2);
    }

    // ── select ───────────────────────────────────────────────────────────────

    #[test]
    fn select_finds_by_label() {
        let mut manifest = WalletManifest::default();
        manifest.upsert(record("main", "abc123"));
        let found = manifest
            .select("main")
            .expect("select by label must succeed");
        assert_eq!(found.wallet_id, "abc123");
    }

    #[test]
    fn select_finds_by_wallet_id() {
        let mut manifest = WalletManifest::default();
        manifest.upsert(record("main", "abc123"));
        let found = manifest
            .select("abc123")
            .expect("select by wallet_id must succeed");
        assert_eq!(found.label, "main");
    }

    #[test]
    fn select_returns_err_for_unknown_selector() {
        let manifest = WalletManifest::default();
        assert!(manifest.select("ghost").is_err());
    }

    // ── load (missing file) ──────────────────────────────────────────────────

    #[test]
    fn load_missing_file_returns_default() {
        let dir = tempfile::tempdir().expect("tempdir must be created");
        let manifest = WalletManifest::load(dir.path()).expect("load of missing file must succeed");
        assert!(
            manifest.wallets.is_empty(),
            "default manifest must have no wallets"
        );
    }

    // ── save + load round-trip ───────────────────────────────────────────────

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir must be created");
        let mut manifest = WalletManifest::default();
        manifest.upsert(WalletRecord {
            label: "main".into(),
            wallet_id: "abc123".into(),
            shielded_address: "0zk1qyfirst000000000000000000000000000000000000000000000000000"
                .into(),
        });
        manifest.upsert(WalletRecord {
            label: "backup".into(),
            wallet_id: "def456".into(),
            shielded_address: "0zk1qysecond00000000000000000000000000000000000000000000000000"
                .into(),
        });
        manifest.save(dir.path()).expect("save must succeed");

        let loaded = WalletManifest::load(dir.path()).expect("load after save must succeed");
        assert_eq!(loaded.wallets.len(), 2);
        assert_eq!(loaded.wallets[0].label, "main");
        assert_eq!(loaded.wallets[0].wallet_id, "abc123");
        assert_eq!(loaded.wallets[1].label, "backup");
        assert_eq!(loaded.wallets[1].wallet_id, "def456");
    }

    // ── validate_label ───────────────────────────────────────────────────────

    #[test]
    fn validate_label_rejects_empty_string() {
        assert!(validate_label("").is_err());
    }

    #[test]
    fn validate_label_rejects_whitespace_only() {
        assert!(validate_label("   ").is_err());
    }

    #[test]
    fn validate_label_accepts_normal_label() {
        assert_eq!(validate_label("main"), Ok("main".to_owned()));
    }
}
