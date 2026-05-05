use alloy_network::{Ethereum, NetworkWallet};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_client::RpcClient;
use anyhow::Result;
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

#[derive(Clone)]
pub struct TorRpcClient {
    tor: ArtiClient,
    rpc_url: Uri,
}

impl TorRpcClient {
    #[must_use]
    pub fn new(tor: ArtiClient, rpc_url: Uri) -> Self {
        Self { tor, rpc_url }
    }

    #[must_use]
    pub fn provider(&self) -> RootProvider<Ethereum> {
        let transport = ArtiJsonRpcTransport::new(self.rpc_url.clone(), self.tor.clone());
        let client = RpcClient::builder().transport(transport, false);
        RootProvider::new(client)
    }

    pub fn wallet_provider(
        &self,
        wallet: impl NetworkWallet<Ethereum> + Clone + 'static,
    ) -> impl Provider<Ethereum> {
        let transport = ArtiJsonRpcTransport::new(self.rpc_url.clone(), self.tor.clone());
        let client = RpcClient::builder().transport(transport, false);
        ProviderBuilder::new().wallet(wallet).connect_client(client)
    }

    /// Send one reverse JSON-RPC request through Tor.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the response is not
    /// successful, or the JSON-RPC response contains an error.
    pub async fn raw_request(&self, method: &str, params: Value) -> Result<Value> {
        tracing::info!(rpc_method = method, "reverse JSON-RPC request through Tor");
        let client: Client<ArtiConnector, Full<Bytes>> =
            Client::builder(TokioExecutor::new()).build(ArtiConnector::new(self.tor.clone()));
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1_u64,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_vec(&body).context("encoding reverse JSON-RPC request")?;
        let request = http::Request::post(self.rpc_url.clone())
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))
            .context("building reverse JSON-RPC request")?;
        let response = client
            .request(request)
            .await
            .context("sending reverse JSON-RPC request through Tor")?;
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

    /// Send one reverse HTTP request through Tor.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL, method, headers, request body, transport, or
    /// response body cannot be processed.
    pub async fn raw_http_request(
        &self,
        request: ReverseHttpRequest,
    ) -> Result<ReverseHttpResponse> {
        let uri: Uri = request.url.parse().context("parsing reverse HTTP URL")?;
        tracing::info!(
            http_method = %request.method,
            http_uri = %uri,
            "reverse HTTP request through Tor"
        );
        let method = Method::from_str(&request.method).context("parsing reverse HTTP method")?;
        let body = match request.body_base64 {
            Some(body) => BASE64
                .decode(body)
                .context("decoding reverse HTTP request body")?,
            None => Vec::new(),
        };
        let client: Client<ArtiConnector, Full<Bytes>> =
            Client::builder(TokioExecutor::new()).build(ArtiConnector::new(self.tor.clone()));
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
            .context("sending reverse HTTP request through Tor")?;
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

    /// Handle one reverse request emitted by the embedded Railgun runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if the reverse request is malformed or the Tor-backed
    /// network request fails.
    pub async fn handle_reverse_request(&self, request: ReverseRequest) -> Result<Value> {
        match request {
            ReverseRequest::JsonRpc { method, params } => self.raw_request(&method, params).await,
            ReverseRequest::Http(request) => self
                .raw_http_request(request)
                .await
                .and_then(|response| serde_json::to_value(response).map_err(Into::into)),
        }
    }
}

#[derive(Debug)]
pub enum ReverseRequest {
    JsonRpc { method: String, params: Value },
    Http(ReverseHttpRequest),
}

impl TryFrom<RawReverseRequest> for ReverseRequest {
    type Error = anyhow::Error;

    fn try_from(request: RawReverseRequest) -> Result<Self> {
        if request.method == "__http_request" {
            let request =
                serde_json::from_value(request.params).context("decoding reverse HTTP request")?;
            Ok(Self::Http(request))
        } else {
            Ok(Self::JsonRpc {
                method: request.method,
                params: request.params,
            })
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ReverseEnvelope {
    pub id: u64,
    #[serde(flatten)]
    request: RawReverseRequest,
}

impl ReverseEnvelope {
    /// Decode the typed request carried by this reverse envelope.
    ///
    /// # Errors
    ///
    /// Returns an error if an HTTP reverse request has malformed parameters.
    pub fn into_request(self) -> Result<(u64, ReverseRequest)> {
        let request = self.request.try_into()?;
        Ok((self.id, request))
    }
}

#[derive(Debug, Deserialize)]
struct RawReverseRequest {
    method: String,
    #[serde(default)]
    params: Value,
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

fn copy_headers(headers: &[(String, String)], target: &mut HeaderMap) -> Result<()> {
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
