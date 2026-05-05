use std::sync::atomic::Ordering;

use alloy_provider::Provider;
use anyhow::Context as _;
use http::Uri;
use undercover::{
    arti::{self, IsolationLabel},
    rpc,
    transport::TOR_CONNECT_CALLS,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let rpc = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://ethereum-sepolia-rpc.publicnode.com".to_owned());
    let rpc_url: Uri = rpc.parse().context("parsing RPC URL")?;

    let state_dir = std::env::temp_dir().join("undercover-spike-arti-state");
    let cache_dir = std::env::temp_dir().join("undercover-spike-arti-cache");
    let tor = arti::bootstrap(&state_dir, &cache_dir)
        .await
        .context("bootstrapping Tor")?;
    let tor = arti::isolated_for(&tor, IsolationLabel::EventSync);
    let provider = rpc::provider(tor, rpc_url);

    let chain_id = provider.get_chain_id().await.context("eth_chainId")?;
    let block_number = provider
        .get_block_number()
        .await
        .context("eth_blockNumber")?;
    let calls = TOR_CONNECT_CALLS.load(Ordering::SeqCst);

    println!("chain_id={chain_id}");
    println!("block_number={block_number}");
    println!("tor_connect_calls={calls}");

    anyhow::ensure!(
        calls > 0,
        "provider call completed without Tor connector use"
    );
    Ok(())
}
