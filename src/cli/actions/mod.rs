use std::path::Path;

use anyhow::Result;
use http::Uri;

use crate::cli::args::{TorArgs, WalletSelectionArgs, WorkdirArgs};
use crate::railgun::manifest::WalletManifest;
use crate::railgun::reverse::ReverseRpcService;
use crate::railgun::{ConnectedRuntime, LoadedWallet, RailgunRuntime, Runtime};
use crate::tor::ArtiClient;

pub mod balance;
pub mod doctor;
pub mod ping;
pub mod shield;
pub mod signer_address;
pub mod unshield;
pub mod wallet;

/// Look up a wallet by label or `wallet_id` and load it into the runtime.
/// Generic over the typestate so callers can pass either a fresh
/// `RailgunRuntime` or an already-connected one.
async fn load_selected_wallet<S>(
    runtime: &mut Runtime<S>,
    workdir: &Path,
    selection: &WalletSelectionArgs,
) -> Result<LoadedWallet> {
    let manifest = WalletManifest::load(workdir)?;
    let record = manifest.select(&selection.wallet)?;
    runtime
        .load_wallet_by_id(&record.wallet_id, &selection.key.encryption_key)
        .await
}

/// Bootstrap an Arti client + reverse-RPC service + connected runtime in
/// one step. Used by shield, balance, and unshield.
async fn bootstrap_connected(
    tor: TorArgs,
    workdir: &WorkdirArgs,
    rpc: Uri,
) -> Result<(ArtiClient, ConnectedRuntime)> {
    let arti = tor.bootstrap_arti().await?;
    let reverse = ReverseRpcService::new(&arti, rpc);
    let runtime = RailgunRuntime::new(&workdir.workdir)
        .await?
        .connect(reverse);
    Ok((arti, runtime))
}
