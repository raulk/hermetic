use alloy_primitives::U256;
use anyhow::Result;
use http::Uri;

use crate::cli::args::{TorArgs, WalletSelectionArgs, WorkdirArgs};
use crate::eth::rpc as eth_rpc;
use crate::eth::signer::{default_signer_address, PublicSignerArgs};
use crate::eth::tx::{parse_populated_transaction, send_transaction};
use crate::railgun::reverse::ReverseRpcService;
use crate::railgun::RailgunRuntime;

use super::load_selected_wallet;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    tor: TorArgs,
    workdir: WorkdirArgs,
    rpc: Uri,
    signer: PublicSignerArgs,
    wallet: WalletSelectionArgs,
    amount_wei: U256,
    recipient: Option<String>,
    dry_run: bool,
) -> Result<()> {
    let arti = tor.bootstrap_arti().await?;
    let reverse = ReverseRpcService::new(arti.clone(), rpc.clone());
    let mut runtime = RailgunRuntime::new(&workdir.workdir)
        .await?
        .connect(reverse);

    let public_wallet = signer.wallet().await?;
    let from = default_signer_address(&public_wallet);
    let recipient = recipient.unwrap_or_else(|| from.to_string());
    let railgun_wallet = load_selected_wallet(&mut runtime, &workdir.workdir, &wallet).await?;
    let populated = runtime
        .prepare_unshield_base_token(
            &railgun_wallet.wallet_id,
            &recipient,
            &wallet.key.encryption_key,
            &amount_wei,
        )
        .await?;
    let tx = parse_populated_transaction(&populated)?;

    println!("wallet_id={}", railgun_wallet.wallet_id);
    println!("shielded_address={}", railgun_wallet.shielded_address);
    println!("to={}", populated.to);
    println!("value={}", populated.value);
    println!("data_len={}", tx.data.len());
    println!("from={from}");
    println!("recipient={recipient}");
    println!("amount_wei={amount_wei}");

    if dry_run {
        return Ok(());
    }

    let provider = eth_rpc::wallet_provider(&arti, rpc, public_wallet);
    send_transaction(provider, from, tx, "unshield base-token transaction").await
}
