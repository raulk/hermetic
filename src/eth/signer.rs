use alloy_network::{Ethereum, EthereumWallet, NetworkWallet};
use alloy_primitives::Address;
use alloy_signer_ledger::{HDPath, LedgerSigner};
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context as _, Result};
use clap::Args;

#[derive(Clone, Debug, Args)]
pub struct PublicSignerArgs {
    #[arg(long, env = "HERMETIC_PRIVATE_KEY", conflicts_with = "ledger")]
    private_key: Option<String>,
    #[arg(long)]
    ledger: bool,
    #[arg(long, default_value_t = crate::eth::network::SEPOLIA_CHAIN_ID)]
    chain_id: u64,
    #[arg(long, default_value_t = 0)]
    ledger_index: usize,
    #[arg(long)]
    ledger_path: Option<String>,
}

impl PublicSignerArgs {
    /// Build an Alloy Ethereum wallet from the selected public signer.
    ///
    /// # Errors
    ///
    /// Returns an error if no signer was selected, private-key parsing fails, or
    /// the Ledger cannot be opened.
    pub async fn wallet(&self) -> Result<EthereumWallet> {
        if let Some(private_key) = &self.private_key {
            let signer: PrivateKeySigner = private_key.parse().context("parsing private key")?;
            return Ok(EthereumWallet::from(signer));
        }
        anyhow::ensure!(
            self.ledger,
            "choose a public signer with --private-key/HERMETIC_PRIVATE_KEY or --ledger"
        );
        let path = self
            .ledger_path
            .as_ref()
            .map(|path| HDPath::Other(path.clone()))
            .unwrap_or(HDPath::LedgerLive(self.ledger_index));
        let signer = LedgerSigner::new(path, Some(self.chain_id))
            .await
            .context("connecting to Ledger Ethereum app")?;
        Ok(EthereumWallet::from(signer))
    }
}

#[must_use]
pub fn default_signer_address(wallet: &EthereumWallet) -> Address {
    <EthereumWallet as NetworkWallet<Ethereum>>::default_signer_address(wallet)
}
