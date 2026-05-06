//! Wire types for reverse requests emitted by the embedded Railgun runtime.
//! The Tor-side servicer lives in `crate::tor::services`.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tor::services::{ReverseHttpRequest, ReverseHttpResponse};

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
