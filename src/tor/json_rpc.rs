use std::task::{Context, Poll};

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use alloy_transport::{TransportError, TransportErrorKind, TransportFut};
use bytes::Bytes;
use http::{Request, Uri};
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::Client;
use tower::Service;

use super::connector::{arti_hyper_client, ArtiConnector};
use super::ArtiClient;

type HyperBody = Full<Bytes>;

#[derive(Clone)]
pub struct ArtiJsonRpcTransport {
    rpc_url: Uri,
    client: Client<ArtiConnector, HyperBody>,
}

impl ArtiJsonRpcTransport {
    pub fn new(rpc_url: Uri, tor: &ArtiClient) -> Self {
        Self {
            rpc_url,
            client: arti_hyper_client(tor),
        }
    }
}

impl Service<RequestPacket> for ArtiJsonRpcTransport {
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: RequestPacket) -> Self::Future {
        let client = self.client.clone();
        let rpc_url = self.rpc_url.clone();

        Box::pin(async move {
            let headers = req.headers();
            let body = req.serialize().map_err(TransportErrorKind::custom)?;
            let mut request = Request::post(rpc_url);
            let request_headers = request.headers_mut().ok_or_else(|| {
                TransportErrorKind::custom_str("RPC request builder has no headers")
            })?;
            request_headers.insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/json"),
            );
            request_headers.extend(headers);
            let request = request
                .body(Full::new(Bytes::copy_from_slice(body.get().as_bytes())))
                .map_err(TransportErrorKind::custom)?;

            let response = client
                .request(request)
                .await
                .map_err(TransportErrorKind::custom)?;
            let status = response.status();
            let bytes = response
                .into_body()
                .collect()
                .await
                .map_err(TransportErrorKind::custom)?
                .to_bytes();

            if !status.is_success() {
                return Err(TransportErrorKind::custom_str(&format!(
                    "RPC HTTP status {status}: {}",
                    String::from_utf8_lossy(&bytes)
                )));
            }

            serde_json::from_slice::<ResponsePacket>(&bytes).map_err(TransportErrorKind::custom)
        })
    }
}
