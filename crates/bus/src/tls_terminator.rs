//! A small TLS terminator in front of a loopback Zenoh listener.
//!
//! Zenoh builds its ServerConfig when the listener starts, so a certificate
//! cannot be replaced under it. This terminator owns the public TLS endpoint,
//! keeps a reloadable certificate resolver, and forwards plaintext to a
//! loopback-only Zenoh listener. Existing connections keep running while a new
//! handshake uses the reloaded certificate.

use std::io::BufReader;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use rustls::crypto::ring::default_provider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ResolvesServerCert, WebPkiClientVerifier};
use rustls::sign::CertifiedKey;
use rustls::{RootCertStore, ServerConfig};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

/// The public TLS endpoint and the loopback listener it forwards to.
#[derive(Debug, Clone)]
pub struct TerminatorConfig {
    /// Address peers connect to.
    pub listen: SocketAddr,
    /// Loopback address of the plaintext Zenoh listener.
    pub backend: SocketAddr,
    /// PEM certificate chain.
    pub certificate: PathBuf,
    /// PEM private key.
    pub private_key: PathBuf,
    /// PEM CA bundle for client certificates.
    pub client_ca: Option<PathBuf>,
    /// Refuse clients that present no trusted certificate.
    pub require_client_certificate: bool,
}

/// A running terminator whose certificate can be reloaded.
///
/// `Clone` shares the same public endpoint; `tokio_rustls::TlsAcceptor` carries
/// no `Debug`, so this type does not derive it either.
#[derive(Clone)]
pub struct TlsTerminator {
    current: Arc<RwLock<Arc<CertifiedKey>>>,
    acceptor: Arc<RwLock<Arc<TlsAcceptor>>>,
    config: TerminatorConfig,
    local_addr: SocketAddr,
    task: tokio::task::AbortHandle,
}

impl TlsTerminator {
    /// Bind the public endpoint and start forwarding.
    pub async fn start(config: TerminatorConfig) -> Result<Self, String> {
        default_provider().install_default().ok();
        let current = Arc::new(RwLock::new(load_key(
            &config.certificate,
            &config.private_key,
        )?));
        let resolver = Arc::new(ReloadingResolver {
            current: Arc::clone(&current),
        });
        let server = build_server_config(&config, resolver)?;
        let acceptor = Arc::new(RwLock::new(Arc::new(TlsAcceptor::from(Arc::new(
            server,
        )))));
        let listener = TcpListener::bind(config.listen)
            .await
            .map_err(|error| error.to_string())?;
        let local_addr = listener.local_addr().map_err(|error| error.to_string())?;
        let backend = config.backend;
        let accept_loop = Arc::clone(&acceptor);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let acceptor = accept_loop
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    let Ok(mut backend) = TcpStream::connect(backend).await else {
                        return;
                    };
                    let _ = copy_bidirectional(&mut tls, &mut backend).await;
                });
            }
        })
        .abort_handle();
        Ok(Self {
            current,
            acceptor,
            config,
            local_addr,
            task,
        })
    }

    /// The bound public address.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Re-read the certificate and key; new handshakes use them immediately.
    pub fn reload(&self) -> Result<(), String> {
        let key = load_key(&self.config.certificate, &self.config.private_key)?;
        let resolver = Arc::new(ReloadingResolver {
            current: Arc::new(RwLock::new(Arc::clone(&key))),
        });
        let server = build_server_config(&self.config, resolver)?;
        *self
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = key;
        *self
            .acceptor
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Arc::new(TlsAcceptor::from(Arc::new(server)));
        Ok(())
    }

    /// Stop accepting new connections.
    pub fn abort(&self) {
        self.task.abort();
    }
}

#[derive(Debug)]
struct ReloadingResolver {
    current: Arc<RwLock<Arc<CertifiedKey>>>,
}

impl ResolvesServerCert for ReloadingResolver {
    fn resolve(&self, _client_hello: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(
            self.current
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        )
    }
}

fn build_server_config(
    config: &TerminatorConfig,
    resolver: Arc<ReloadingResolver>,
) -> Result<ServerConfig, String> {
    let verifier = if config.require_client_certificate {
        let path = config
            .client_ca
            .as_ref()
            .ok_or_else(|| "mTLS needs a client CA bundle".to_string())?;
        let mut roots = RootCertStore::empty();
        for cert in load_certs(path)? {
            roots.add(cert).map_err(|error| error.to_string())?;
        }
        Some(
            WebPkiClientVerifier::builder(Arc::new(roots))
                .build()
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    let builder = match verifier {
        Some(verifier) => ServerConfig::builder().with_client_cert_verifier(verifier),
        None => ServerConfig::builder().with_no_client_auth(),
    };
    let mut server = builder.with_cert_resolver(resolver);
    server.alpn_protocols.clear();
    Ok(server)
}

fn load_key(certificate: &Path, private_key: &Path) -> Result<Arc<CertifiedKey>, String> {
    let certs = load_certs(certificate)?;
    let mut reader =
        BufReader::new(std::fs::File::open(private_key).map_err(|error| error.to_string())?);
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut reader)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "the TLS key file has no private key".to_string())?;
    CertifiedKey::from_der(certs, key, &default_provider())
        .map(Arc::new)
        .map_err(|error| error.to_string())
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let mut reader = BufReader::new(std::fs::File::open(path).map_err(|error| error.to_string())?);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
    use rustls::pki_types::ServerName;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::TlsConnector;

    struct Material {
        ca: PathBuf,
        cert: PathBuf,
        key: PathBuf,
        root: PathBuf,
    }

    fn material(label: &str) -> Material {
        let root = std::env::temp_dir().join(format!("bus-term-{}-{label}", std::process::id()));
        std::fs::create_dir_all(&root).expect("root");
        Material {
            ca: root.join("ca.pem"),
            cert: root.join("server.pem"),
            key: root.join("server.key"),
            root,
        }
    }

    fn issue(name: &str) -> (String, String, String) {
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, format!("{name} ca"));
        let ca_key = KeyPair::generate().expect("ca key");
        let ca = ca_params.self_signed(&ca_key).expect("ca");
        let mut params = CertificateParams::new(vec!["localhost".to_string()]).expect("params");
        params.distinguished_name.push(DnType::CommonName, name);
        let key = KeyPair::generate().expect("key");
        let cert = params.signed_by(&key, &ca, &ca_key).expect("cert");
        (ca.pem(), cert.pem(), key.serialize_pem())
    }

    async fn client(
        addr: SocketAddr,
        ca: &Path,
    ) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
        let mut roots = RootCertStore::empty();
        for cert in load_certs(ca).expect("ca") {
            roots.add(cert).expect("root");
        }
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));
        let tcp = tokio::net::TcpStream::connect(addr).await.expect("tcp");
        connector
            .connect(ServerName::try_from("localhost").expect("name"), tcp)
            .await
            .expect("tls")
    }

    async fn echo_backend() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("backend");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.expect("accept");
                tokio::spawn(async move {
                    let mut buf = [0u8; 64];
                    while let Ok(read) = stream.read(&mut buf).await {
                        if read == 0 || stream.write_all(&buf[..read]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        addr
    }

    async fn round_trip(
        stream: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
        byte: u8,
    ) {
        stream.write_all(&[byte]).await.expect("write");
        let mut buffer = [0u8; 1];
        stream.read_exact(&mut buffer).await.expect("read");
        assert_eq!(buffer[0], byte);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_reload_keeps_existing_connections_and_uses_the_new_certificate() {
        let material = material("reload");
        let (ca1, cert1, key1) = issue("first");
        std::fs::write(&material.ca, ca1).expect("ca1");
        std::fs::write(&material.cert, cert1).expect("cert1");
        std::fs::write(&material.key, key1).expect("key1");
        let backend = echo_backend().await;
        let terminator = TlsTerminator::start(TerminatorConfig {
            listen: "127.0.0.1:0".parse().expect("listen"),
            backend,
            certificate: material.cert.clone(),
            private_key: material.key.clone(),
            client_ca: None,
            require_client_certificate: false,
        })
        .await
        .expect("terminator");
        let addr = terminator.local_addr();

        let mut old = client(addr, &material.ca).await;
        round_trip(&mut old, b'a').await;

        let (ca2, cert2, key2) = issue("second");
        std::fs::write(&material.cert, cert2).expect("cert2");
        std::fs::write(&material.key, key2).expect("key2");
        terminator.reload().expect("reload");

        // The established connection is not interrupted by the rotation.
        round_trip(&mut old, b'b').await;

        // A new handshake uses the new certificate, so its client trusts CA2.
        std::fs::write(material.root.join("ca2.pem"), ca2).expect("ca2");
        let mut fresh = client(addr, &material.root.join("ca2.pem")).await;
        round_trip(&mut fresh, b'c').await;

        terminator.abort();
        let _ = std::fs::remove_dir_all(material.root);
    }
}
