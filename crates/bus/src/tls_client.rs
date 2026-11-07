//! A reloadable TLS client proxy in front of a loopback Zenoh connector.
//!
//! Zenoh builds its client config when a session starts. BSH connects to this
//! proxy over loopback and the proxy performs the TLS handshake with a client
//! config that can be replaced without touching established connections.

use std::io::BufReader;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use rustls::crypto::ring::default_provider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;

/// The local plaintext endpoint and the remote TLS server it reaches.
#[derive(Debug, Clone)]
pub struct ClientTerminatorConfig {
    /// Loopback address BSH's Zenoh session connects to.
    pub listen: SocketAddr,
    /// Remote TLS endpoint.
    pub remote: SocketAddr,
    /// Certificate name to verify.
    pub server_name: String,
    /// PEM CA bundle for the remote server.
    pub root_ca: PathBuf,
    /// PEM client certificate chain.
    pub certificate: Option<PathBuf>,
    /// PEM client private key.
    pub private_key: Option<PathBuf>,
}

/// A running client proxy whose client certificate can be reloaded.
#[derive(Clone)]
pub struct TlsClientTerminator {
    config: ClientTerminatorConfig,
    current: Arc<RwLock<Arc<ClientConfig>>>,
    local_addr: SocketAddr,
    task: tokio::task::AbortHandle,
}

impl TlsClientTerminator {
    /// Bind the loopback endpoint and start forwarding.
    pub async fn start(config: ClientTerminatorConfig) -> Result<Self, String> {
        default_provider().install_default().ok();
        let current = Arc::new(RwLock::new(Arc::new(build_client_config(&config)?)));
        let listener = TcpListener::bind(config.listen)
            .await
            .map_err(|error| error.to_string())?;
        let local_addr = listener.local_addr().map_err(|error| error.to_string())?;
        let shared = Arc::clone(&current);
        let remote = config.remote;
        let server_name = config.server_name.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut local, _)) = listener.accept().await else {
                    continue;
                };
                let tls = shared
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                let name = server_name.clone();
                tokio::spawn(async move {
                    let Ok(remote_stream) = TcpStream::connect(remote).await else {
                        return;
                    };
                    let Ok(name) = ServerName::try_from(name) else {
                        return;
                    };
                    let Ok(mut tls_stream) =
                        TlsConnector::from(tls).connect(name, remote_stream).await
                    else {
                        return;
                    };
                    let _ = copy_bidirectional(&mut local, &mut tls_stream).await;
                });
            }
        })
        .abort_handle();
        Ok(Self {
            config,
            current,
            local_addr,
            task,
        })
    }

    /// The loopback address Zenoh should connect to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Reload the CA and client certificate for new connections.
    pub fn reload(&self) -> Result<(), String> {
        let fresh = Arc::new(build_client_config(&self.config)?);
        *self
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = fresh;
        Ok(())
    }

    /// Stop accepting new local connections.
    pub fn abort(&self) {
        self.task.abort();
    }
}

fn build_client_config(config: &ClientTerminatorConfig) -> Result<ClientConfig, String> {
    let mut roots = RootCertStore::empty();
    for cert in load_certs(&config.root_ca)? {
        roots.add(cert).map_err(|error| error.to_string())?;
    }
    let builder = ClientConfig::builder().with_root_certificates(roots);
    match (&config.certificate, &config.private_key) {
        (Some(certificate), Some(private_key)) => {
            let certs = load_certs(certificate)?;
            let mut reader = BufReader::new(
                std::fs::File::open(private_key).map_err(|error| error.to_string())?,
            );
            let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut reader)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "the TLS key file has no private key".to_string())?;
            builder
                .with_client_auth_cert(certs, key)
                .map_err(|error| error.to_string())
        }
        _ => Ok(builder.with_no_client_auth()),
    }
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let mut reader = BufReader::new(std::fs::File::open(path).map_err(|error| error.to_string())?);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}
