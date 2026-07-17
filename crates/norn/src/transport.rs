//! Transport: open a TCP socket, optionally wrapped in TLS.
//!
//! Returns a boxed stream so the plaintext and TLS cases share one type. TLS
//! uses rustls with the Mozilla root set (webpki-roots) and the ring crypto
//! provider, supplied explicitly so no process-global default has to be
//! installed.

use std::io;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

use crate::config::ConnConfig;

/// Any byte stream we can run IRC over.
pub trait IrcStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> IrcStream for T {}

/// A boxed connection stream: plaintext TCP or TLS over TCP.
pub type Stream = Box<dyn IrcStream>;

/// Connect to `cfg.host:cfg.port`, wrapping in TLS unless `cfg.tls` is false.
pub async fn connect(cfg: &ConnConfig) -> io::Result<Stream> {
    let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
    tcp.set_nodelay(true).ok();

    if !cfg.tls {
        return Ok(Box::new(tcp));
    }

    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_root_certificates(roots)
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));
    let server_name = ServerName::try_from(cfg.host.clone())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid TLS server name"))?;

    let tls = connector.connect(server_name, tcp).await?;
    Ok(Box::new(tls))
}
