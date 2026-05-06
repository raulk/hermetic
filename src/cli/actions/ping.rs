use alloy_provider::Provider;
use anyhow::{Context as _, Result};
use http::Uri;

use crate::cli::args::TorArgs;
use crate::eth::rpc as eth_rpc;

pub(crate) async fn run(tor: TorArgs, rpc_url: Uri) -> Result<()> {
    let arti = tor.bootstrap_arti().await?;
    let provider = eth_rpc::provider(&arti, rpc_url);
    let chain_id = provider.get_chain_id().await.context("eth_chainId")?;
    let block_number = provider
        .get_block_number()
        .await
        .context("eth_blockNumber")?;

    println!("chain_id={chain_id}");
    println!("block_number={block_number}");
    Ok(())
}
