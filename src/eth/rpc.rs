//! Alloy provider construction over the Tor-backed JSON-RPC transport.

use alloy_network::{Ethereum, NetworkWallet};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_client::RpcClient;
use http::Uri;

use crate::tor::json_rpc::ArtiJsonRpcTransport;
use crate::tor::ArtiClient;

#[must_use]
pub fn provider(arti: &ArtiClient, rpc_url: Uri) -> RootProvider<Ethereum> {
    let transport = ArtiJsonRpcTransport::new(rpc_url, arti.clone());
    let client = RpcClient::builder().transport(transport, false);
    RootProvider::new(client)
}

pub fn wallet_provider<W>(arti: &ArtiClient, rpc_url: Uri, wallet: W) -> impl Provider<Ethereum>
where
    W: NetworkWallet<Ethereum> + Clone + 'static,
{
    let transport = ArtiJsonRpcTransport::new(rpc_url, arti.clone());
    let client = RpcClient::builder().transport(transport, false);
    ProviderBuilder::new().wallet(wallet).connect_client(client)
}
