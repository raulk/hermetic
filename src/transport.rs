use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

use alloy_json_rpc::{RequestPacket, ResponsePacket};
use alloy_transport::{TransportError, TransportErrorKind, TransportFut};
use anyhow::{anyhow, Context as _};
use arti_client::IntoTorAddr;
use bytes::Bytes;
use http::{Request, Uri};
use http_body_util::{BodyExt, Full};
use hyper_util::{
    client::legacy::{connect::Connected, Client},
    rt::{TokioExecutor, TokioIo},
};
use rustls::{pki_types::ServerName, RootCertStore};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::{client::TlsStream, TlsConnector};
use tower::Service;

use crate::arti::ArtiClient;

pub static ARTI_CONNECT_CALLS: AtomicUsize = AtomicUsize::new(0);
static ARTI_CONNECTION_IDS: AtomicUsize = AtomicUsize::new(0);

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type HyperBody = Full<Bytes>;

#[derive(Clone)]
pub struct ArtiConnector {
    tor: ArtiClient,
    tls: TlsConnector,
}

impl ArtiConnector {
    pub fn new(tor: ArtiClient) -> Self {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();

        Self {
            tor,
            tls: TlsConnector::from(Arc::new(tls_config)),
        }
    }
}

impl Service<Uri> for ArtiConnector {
    type Response = TokioIo<ArtiTlsStream>;
    type Error = anyhow::Error;
    type Future = BoxFuture<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let tor = self.tor.clone();
        let tls = self.tls.clone();

        Box::pin(async move {
            let scheme = uri.scheme_str().unwrap_or("https");
            if scheme != "https" {
                return Err(anyhow!("only https RPC URLs are allowed"));
            }

            let host = uri
                .host()
                .ok_or_else(|| anyhow!("RPC URI does not contain a host"))?
                .to_owned();
            let port = uri.port_u16().unwrap_or(443);

            let connection_id = ARTI_CONNECTION_IDS.fetch_add(1, Ordering::SeqCst) + 1;
            ARTI_CONNECT_CALLS.fetch_add(1, Ordering::SeqCst);
            tracing::info!(
                connection_id,
                rpc_scheme = scheme,
                rpc_host = %host,
                rpc_port = port,
                "opening RPC stream through Arti"
            );
            let stream = tor
                .connect((host.as_str(), port).into_tor_addr()?)
                .await
                .with_context(|| format!("connecting to {host}:{port} through Arti"))?;
            tracing::info!(
                connection_id,
                rpc_host = %host,
                rpc_port = port,
                "Arti stream established; circuit path is not exposed by arti-client DataStream"
            );

            let server_name = ServerName::try_from(host)
                .map_err(|_| anyhow!("RPC URI host is not a valid TLS server name"))?;
            let tls_stream = tls
                .connect(server_name, stream)
                .await
                .context("performing rustls handshake over Arti stream")?;
            tracing::info!(connection_id, "TLS handshake completed over Arti stream");

            Ok(TokioIo::new(ArtiTlsStream(tls_stream)))
        })
    }
}

pub struct ArtiTlsStream(TlsStream<arti_client::DataStream>);

impl hyper_util::client::legacy::connect::Connection for ArtiTlsStream {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

impl AsyncRead for ArtiTlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

impl AsyncWrite for ArtiTlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
    }
}

#[derive(Clone)]
pub struct ArtiJsonRpcTransport {
    rpc_url: Uri,
    client: Client<ArtiConnector, HyperBody>,
}

impl ArtiJsonRpcTransport {
    pub fn new(rpc_url: Uri, tor: ArtiClient) -> Self {
        let connector = ArtiConnector::new(tor);
        let client = Client::builder(TokioExecutor::new()).build(connector);
        Self { rpc_url, client }
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
            let body = req.serialize().map_err(TransportErrorKind::custom)?;
            let request = Request::post(rpc_url)
                .header(http::header::CONTENT_TYPE, "application/json")
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
