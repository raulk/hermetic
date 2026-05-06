//! Alloy provider construction over the Tor-backed JSON-RPC transport.

use alloy_network::{Ethereum, NetworkWallet};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_client::RpcClient;
use http::Uri;

use crate::tor::json_rpc::ArtiJsonRpcTransport;
use crate::tor::ArtiClient;

fn build_client(arti: &ArtiClient, rpc_url: Uri) -> RpcClient {
    let transport = ArtiJsonRpcTransport::new(rpc_url, arti);
    RpcClient::builder().transport(transport, false)
}

#[must_use]
pub fn provider(arti: &ArtiClient, rpc_url: Uri) -> RootProvider<Ethereum> {
    RootProvider::new(build_client(arti, rpc_url))
}

pub fn wallet_provider<W>(arti: &ArtiClient, rpc_url: Uri, wallet: W) -> impl Provider<Ethereum>
where
    W: NetworkWallet<Ethereum> + Clone + 'static,
{
    ProviderBuilder::new()
        .wallet(wallet)
        .connect_client(build_client(arti, rpc_url))
}
