use alloy_network::{Ethereum, NetworkWallet};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_client::RpcClient;
use anyhow::{anyhow, Context as _};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Uri};
use http_body_util::{BodyExt, Full};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;

use crate::{
    arti::ArtiClient,
    transport::{ArtiConnector, ArtiJsonRpcTransport},
};

pub fn provider(tor: ArtiClient, rpc_url: Uri) -> RootProvider<Ethereum> {
    let transport = ArtiJsonRpcTransport::new(rpc_url, tor);
    let client = RpcClient::builder().transport(transport, false);
    RootProvider::new(client)
}

pub fn wallet_provider(
    tor: ArtiClient,
    rpc_url: Uri,
    wallet: impl NetworkWallet<Ethereum> + Clone + 'static,
) -> impl Provider<Ethereum> {
    let transport = ArtiJsonRpcTransport::new(rpc_url, tor);
    let client = RpcClient::builder().transport(transport, false);
    ProviderBuilder::new().wallet(wallet).connect_client(client)
}

pub async fn raw_request(
    tor: ArtiClient,
    rpc_url: Uri,
    method: &str,
    params: Value,
) -> anyhow::Result<Value> {
    tracing::info!(rpc_method = method, "reverse JSON-RPC request through Arti");
    let client: Client<ArtiConnector, Full<Bytes>> =
        Client::builder(TokioExecutor::new()).build(ArtiConnector::new(tor));
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1_u64,
        "method": method,
        "params": params,
    });
    let body = serde_json::to_vec(&body).context("encoding reverse JSON-RPC request")?;
    let request = http::Request::post(rpc_url)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)))
        .context("building reverse JSON-RPC request")?;
    let response = client
        .request(request)
        .await
        .context("sending reverse JSON-RPC request through Arti")?;
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .context("reading reverse JSON-RPC response")?
        .to_bytes();
    if !status.is_success() {
        return Err(anyhow!(
            "reverse JSON-RPC HTTP status {status}: {}",
            String::from_utf8_lossy(&bytes)
        ));
    }
    let response: Value =
        serde_json::from_slice(&bytes).context("decoding reverse JSON-RPC response")?;
    if let Some(error) = response.get("error") {
        return Err(anyhow!("reverse JSON-RPC error: {error}"));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("reverse JSON-RPC response had no result"))
}

#[derive(Debug, Deserialize)]
pub struct ReverseHttpRequest {
    pub url: String,
    pub method: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body_base64: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ReverseHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body_base64: String,
}

pub async fn raw_http_request(
    tor: ArtiClient,
    request: ReverseHttpRequest,
) -> anyhow::Result<ReverseHttpResponse> {
    let uri: Uri = request.url.parse().context("parsing reverse HTTP URL")?;
    tracing::info!(
        http_method = %request.method,
        http_uri = %uri,
        "reverse HTTP request through Arti"
    );
    let method = Method::from_str(&request.method).context("parsing reverse HTTP method")?;
    let body = match request.body_base64 {
        Some(body) => BASE64
            .decode(body)
            .context("decoding reverse HTTP request body")?,
        None => Vec::new(),
    };
    let client: Client<ArtiConnector, Full<Bytes>> =
        Client::builder(TokioExecutor::new()).build(ArtiConnector::new(tor));
    let mut builder = http::Request::builder().method(method).uri(uri);
    let headers = builder
        .headers_mut()
        .ok_or_else(|| anyhow!("reverse HTTP request builder has no headers"))?;
    copy_headers(&request.headers, headers)?;
    let request = builder
        .body(Full::new(Bytes::from(body)))
        .context("building reverse HTTP request")?;
    let response = client
        .request(request)
        .await
        .context("sending reverse HTTP request through Arti")?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
        })
        .collect();
    let body = response
        .into_body()
        .collect()
        .await
        .context("reading reverse HTTP response")?
        .to_bytes();
    Ok(ReverseHttpResponse {
        status,
        headers,
        body_base64: BASE64.encode(body),
    })
}

fn copy_headers(headers: &[(String, String)], target: &mut HeaderMap) -> anyhow::Result<()> {
    for (name, value) in headers {
        let name = HeaderName::from_str(name).with_context(|| format!("invalid header {name}"))?;
        if name == http::header::HOST || name == http::header::CONNECTION {
            continue;
        }
        let value =
            HeaderValue::from_str(value).with_context(|| format!("invalid value for {name}"))?;
        target.insert(name, value);
    }
    Ok(())
}
