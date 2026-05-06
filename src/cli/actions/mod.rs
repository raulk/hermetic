#![allow(clippy::missing_errors_doc)]

use std::path::Path;

use anyhow::Result;

use crate::cli::args::WalletSelectionArgs;
use crate::railgun::manifest::WalletManifest;
use crate::railgun::{LoadedWallet, RailgunRuntime};

pub mod balance;
pub mod doctor;
pub mod ping;
pub mod shield;
pub mod signer_address;
pub mod unshield;
pub mod wallet;

/// Look up a wallet by label or `wallet_id` and load it into the runtime.
async fn load_selected_wallet(
    runtime: &mut RailgunRuntime,
    workdir: &Path,
    selection: &WalletSelectionArgs,
) -> Result<LoadedWallet> {
    let manifest = WalletManifest::load(workdir)?;
    let record = manifest.select(&selection.wallet)?;
    runtime
        .load_wallet_by_id(&record.wallet_id, &selection.key.encryption_key)
        .await
}
