use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

use anyhow::{anyhow, Context as _};
use arti_client::IntoTorAddr;
use bytes::Bytes;
use http::Uri;
use http_body_util::Full;
use hyper_util::client::legacy::connect::Connected;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use tower::Service;

use super::ArtiClient;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Build a hyper Client wired to a fresh `ArtiConnector`. The connector
/// holds the per-process cached `ClientConfig` so the cert store is not
/// rebuilt per call.
#[must_use]
pub fn arti_hyper_client(arti: &ArtiClient) -> Client<ArtiConnector, Full<Bytes>> {
    Client::builder(TokioExecutor::new()).build(ArtiConnector::new(arti.clone()))
}

fn shared_client_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            )
        })
        .clone()
}

#[derive(Clone)]
pub struct ArtiConnector {
    tor: ArtiClient,
    tls: TlsConnector,
}

impl ArtiConnector {
    pub fn new(tor: ArtiClient) -> Self {
        Self {
            tor,
            tls: TlsConnector::from(shared_client_config()),
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

            tracing::info!(
                rpc_scheme = scheme,
                rpc_host = %host,
                rpc_port = port,
                "opening RPC stream through Tor"
            );
            let stream = tor
                .connect((host.as_str(), port).into_tor_addr()?)
                .await
                .with_context(|| format!("connecting to {host}:{port} through Tor"))?;
            tracing::info!(
                rpc_host = %host,
                rpc_port = port,
                "Tor stream established; circuit path is not exposed by arti-client DataStream"
            );

            let server_name = ServerName::try_from(host)
                .map_err(|_| anyhow!("RPC URI host is not a valid TLS server name"))?;
            let tls_stream = tls
                .connect(server_name, stream)
                .await
                .context("performing rustls handshake over Tor stream")?;
            tracing::info!("TLS handshake completed over Tor stream");

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
