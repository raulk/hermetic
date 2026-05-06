use alloy_network::{Ethereum, NetworkWallet};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_client::RpcClient;
use anyhow::{anyhow, Context as _, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Uri};
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde_json::value::to_raw_value;
use serde_json::Value;
use std::borrow::Cow;
use std::str::FromStr;

use crate::railgun::reverse::{ReverseRequest, ReverseResponse};
use crate::tor::connector::ArtiConnector;
use crate::tor::json_rpc::ArtiJsonRpcTransport;
use crate::tor::services::{self, ReverseHttpRequest, ReverseHttpResponse};
use crate::tor::ArtiClient;

#[derive(Clone)]
pub struct TorRpcClient {
    tor: ArtiClient,
    rpc_url: Uri,
    client: Client<ArtiConnector, Full<Bytes>>,
}

impl TorRpcClient {
    #[must_use]
    pub fn new(tor: ArtiClient, rpc_url: Uri) -> Self {
        let connector = ArtiConnector::new(tor.clone());
        let client = Client::builder(TokioExecutor::new()).build(connector);
        Self {
            tor,
            rpc_url,
            client,
        }
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
    /// Returns an error if Alloy cannot encode, send, or decode the JSON-RPC
    /// request.
    pub async fn raw_request(&self, method: &str, params: Value) -> Result<Value> {
        tracing::info!(rpc_method = method, "reverse JSON-RPC request through Tor");
        let params = to_raw_value(&params).context("encoding reverse JSON-RPC params")?;
        let result = self
            .provider()
            .raw_request_dyn(Cow::Owned(method.to_owned()), &params)
            .await
            .context("sending reverse JSON-RPC request through Tor")?;
        serde_json::from_str(result.get()).context("decoding reverse JSON-RPC result")
    }

    /// Send one reverse HTTP request through Tor.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL, method, headers, request body, transport, or
    /// response body cannot be processed.
    pub async fn raw_service_http_request(
        &self,
        request: ReverseHttpRequest,
    ) -> Result<ReverseHttpResponse> {
        let uri = services::service_uri(&request.service, request.path.as_deref())?;
        tracing::info!(
            http_method = %request.method,
            http_service = %request.service,
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
        let client = self.client.clone();
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
    pub async fn handle_reverse_request(&self, request: ReverseRequest) -> Result<ReverseResponse> {
        match request {
            ReverseRequest::JsonRpc { method, params } => self
                .raw_request(&method, params)
                .await
                .map(ReverseResponse::JsonRpc),
            ReverseRequest::ServiceHttp { request } => self
                .raw_service_http_request(request)
                .await
                .map(ReverseResponse::Http),
        }
    }
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
