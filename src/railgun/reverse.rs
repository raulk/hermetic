use anyhow::{anyhow, Context as _, Result};
use http::Uri;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const REVERSE_HTTP_SERVICES: &[ReverseHttpService] = &[
    ReverseHttpService {
        name: "graphql",
        origin: "https://rail-squid.squids.live",
        default_path: "/squid-railgun-eth-sepolia-v2/graphql",
        endpoint: "https://rail-squid.squids.live/squid-railgun-eth-sepolia-v2/graphql",
    },
    ReverseHttpService {
        name: "poi",
        origin: "https://ppoi-agg.horsewithsixlegs.xyz",
        default_path: "/",
        endpoint: "https://ppoi-agg.horsewithsixlegs.xyz",
    },
];

struct ReverseHttpService {
    name: &'static str,
    origin: &'static str,
    default_path: &'static str,
    endpoint: &'static str,
}

/// Return a Rust-owned reverse HTTP endpoint by service name.
///
/// # Errors
///
/// Returns an error if `name` is not a known reverse HTTP service.
pub fn service_endpoint(name: &str) -> Result<&'static str> {
    REVERSE_HTTP_SERVICES
        .iter()
        .find(|service| service.name == name)
        .map(|service| service.endpoint)
        .ok_or_else(|| anyhow!("unknown reverse HTTP service endpoint: {name}"))
}

/// Compose an allowlisted Railgun reverse-service URI.
///
/// # Errors
///
/// Returns an error if the service is unknown or the path is invalid.
pub fn service_uri(name: &str, path: Option<&str>) -> Result<Uri> {
    let service = REVERSE_HTTP_SERVICES
        .iter()
        .find(|service| service.name == name)
        .ok_or_else(|| anyhow!("unknown reverse HTTP service: {name}"))?;
    let path = path.unwrap_or(service.default_path);
    anyhow::ensure!(
        path.starts_with('/'),
        "reverse HTTP service path must start with /"
    );
    format!("{}{}", service.origin, path)
        .parse()
        .context("parsing reverse HTTP service URI")
}

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

#[derive(Debug, Deserialize)]
pub struct ReverseHttpRequest {
    pub service: String,
    #[serde(default)]
    pub path: Option<String>,
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
