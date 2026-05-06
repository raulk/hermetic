use anyhow::Result;
use http::Uri;

use crate::cli::args::{TorArgs, WalletSelectionArgs, WorkdirArgs};
use crate::railgun::RailgunRuntime;

use super::load_selected_wallet;

pub async fn run(
    tor: TorArgs,
    workdir: WorkdirArgs,
    rpc: Uri,
    wallet: WalletSelectionArgs,
) -> Result<()> {
    let rpc_client = tor.bootstrap_rpc_client(rpc).await?;
    let mut runtime = RailgunRuntime::new(&workdir.workdir)
        .await?
        .with_rpc_client(rpc_client);

    let railgun_wallet = load_selected_wallet(&mut runtime, &workdir.workdir, &wallet).await?;
    let refreshed = runtime.refresh_balance(&railgun_wallet.wallet_id).await?;

    println!("wallet_id={}", railgun_wallet.wallet_id);
    println!("shielded_address={}", railgun_wallet.shielded_address);
    println!("token_address={}", refreshed.token_address);
    println!("balance={}", refreshed.balance);
    println!("spendable_balance={}", refreshed.spendable_balance);
    Ok(())
}
