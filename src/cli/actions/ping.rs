use alloy_provider::Provider;
use anyhow::{Context as _, Result};
use http::Uri;

use crate::cli::args::TorArgs;

pub async fn run(tor: TorArgs, rpc_url: Uri) -> Result<()> {
    let rpc_client = tor.bootstrap_rpc_client(rpc_url).await?;
    let provider = rpc_client.provider();
    let chain_id = provider.get_chain_id().await.context("eth_chainId")?;
    let block_number = provider
        .get_block_number()
        .await
        .context("eth_blockNumber")?;

    println!("chain_id={chain_id}");
    println!("block_number={block_number}");
    Ok(())
}
