//! Wire types and Tor-backed servicer for reverse requests emitted by the
//! embedded Railgun runtime.

use std::borrow::Cow;
use std::str::FromStr;

use alloy_network::Ethereum;
use alloy_provider::{Provider, RootProvider};
use anyhow::{anyhow, Context as _, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Uri};
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::Client;
use serde::{Deserialize, Serialize};
use serde_json::value::to_raw_value;
use serde_json::Value;

use crate::eth::rpc as eth_rpc;
use crate::tor::connector::{arti_hyper_client, ArtiConnector};
use crate::tor::services::{self, ReverseHttpRequest, ReverseHttpResponse};
use crate::tor::ArtiClient;

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ReverseResponse {
    JsonRpc(Value),
    Http(ReverseHttpResponse),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReverseRequest {
    JsonRpc {
        method: String,
        #[serde(default)]
        params: Value,
    },
    ServiceHttp {
        #[serde(flatten)]
        request: ReverseHttpRequest,
    },
}

impl TryFrom<Value> for ReverseRequest {
    type Error = anyhow::Error;

    fn try_from(value: Value) -> Result<Self> {
        serde_json::from_value(value).context("decoding reverse request")
    }
}

/// Services reverse JSON-RPC and reverse HTTP requests from the embedded
/// runtime through Rust-owned Tor egress. One instance per command; both
/// the Alloy provider and the hyper Client are cached so connection
/// pooling amortizes across the many requests a single Railgun call makes.
#[derive(Clone)]
pub struct ReverseRpcService {
    provider: RootProvider<Ethereum>,
    client: Client<ArtiConnector, Full<Bytes>>,
}

impl ReverseRpcService {
    #[must_use]
    pub fn new(arti: &ArtiClient, rpc_url: Uri) -> Self {
        Self {
            provider: eth_rpc::provider(arti, rpc_url),
            client: arti_hyper_client(arti),
        }
    }

    /// Service one reverse request emitted by the embedded Railgun runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if the reverse request is malformed or the
    /// Tor-backed network request fails.
    pub async fn handle(&self, request: ReverseRequest) -> Result<ReverseResponse> {
        match request {
            ReverseRequest::JsonRpc { method, params } => self
                .json_rpc(&method, params)
                .await
                .map(ReverseResponse::JsonRpc),
            ReverseRequest::ServiceHttp { request } => {
                self.service_http(request).await.map(ReverseResponse::Http)
            }
        }
    }

    async fn json_rpc(&self, method: &str, params: Value) -> Result<Value> {
        tracing::info!(rpc_method = method, "reverse JSON-RPC request through Tor");
        let params = to_raw_value(&params).context("encoding reverse JSON-RPC params")?;
        let result = self
            .provider
            .raw_request_dyn(Cow::Owned(method.to_owned()), &params)
            .await
            .context("sending reverse JSON-RPC request through Tor")?;
        serde_json::from_str(result.get()).context("decoding reverse JSON-RPC result")
    }

    async fn service_http(&self, request: ReverseHttpRequest) -> Result<ReverseHttpResponse> {
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
        let mut builder = http::Request::builder().method(method).uri(uri);
        let headers = builder
            .headers_mut()
            .ok_or_else(|| anyhow!("reverse HTTP request builder has no headers"))?;
        copy_headers(&request.headers, headers)?;
        let request = builder
            .body(Full::new(Bytes::from(body)))
            .context("building reverse HTTP request")?;
        let response = self
            .client
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
