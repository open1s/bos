//! Data-plane streaming: `read_stream`/`write_stream` through the stack.
//!
//! Two layers are exercised:
//! - in-process: `ResourceClient -> ResourceManager -> FileResource`
//! - wire: the same, but over `QuicTransport`/`QuicServer` (mTLS)
//!
//! The QUIC tests verify the protocol transfers data *incrementally* (chunks
//! arrive before the whole file is transmitted) and that content is exact.

use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use rcgen::{BasicConstraints, CertificateParams, CertifiedKey, DnType, IsCa, KeyPair};
use resource::prelude::*;
use resource::transport::{QuicServer, QuicTransport};
use resource::{ChunkWriter, Effect, PolicyDoc, Rule};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

fn policy() -> SharedPolicy {
    SharedPolicy::new(PolicyDoc {
        admins: vec!["admin".to_string()],
        rules: vec![Rule {
            agents: vec!["agent1".to_string()],
            uris: vec!["file://*".to_string()],
            actions: vec!["*".to_string()],
            effect: Effect::Allow,
        }],
        ..Default::default()
    })
}

fn gen_ca() -> CertifiedKey {
    let mut params = CertificateParams::new(vec!["BOS-CA".to_string()]).unwrap();
    params.distinguished_name.push(DnType::CommonName, "BOS-CA");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let kp = KeyPair::generate().unwrap();
    let cert = params.self_signed(&kp).expect("self-sign CA");
    CertifiedKey { cert, key_pair: kp }
}

fn gen_entity(ca: &CertifiedKey, cn: &str) -> CertifiedKey {
    let kp = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec![cn.to_string()]).unwrap();
    params.distinguished_name.push(DnType::CommonName, cn);
    let cert = params
        .signed_by(&kp, &ca.cert, &ca.key_pair)
        .expect("sign entity cert");
    CertifiedKey { cert, key_pair: kp }
}

fn cert_der(c: &CertifiedKey) -> CertificateDer<'static> {
    CertificateDer::from(c.cert.der().as_ref().to_vec())
}

fn key_der(c: &CertifiedKey) -> PrivateKeyDer<'static> {
    PrivateKeyDer::from(PrivatePkcs8KeyDer::from(c.key_pair.serialize_der()))
}

fn ca_pem(c: &CertifiedKey) -> Vec<u8> {
    c.cert.pem().into_bytes()
}

/// Spin up a QUIC server backed by `mgr`; return a connected client.
async fn connected_client(mgr: Arc<ResourceManager>) -> ResourceClient {
    let ca = gen_ca();
    let server_k = gen_entity(&ca, "server");
    let client_k = gen_entity(&ca, "agent1");

    let server = Arc::new(QuicServer::new(mgr));
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let local = server
        .bind(
            addr,
            cert_der(&server_k),
            key_der(&server_k),
            &ca_pem(&ca),
            &[],
        )
        .await
        .expect("bind server");
    let srv = server.clone();
    tokio::spawn(async move {
        let _ = srv.run().await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let trust = ca_pem(&ca);
    let transport = Arc::new(
        QuicTransport::new(
            local,
            "server".to_string(),
            &trust,
            cert_der(&client_k),
            key_der(&client_k),
        )
        .expect("build client transport"),
    );
    ResourceClient::new("agent1", None, Some(transport))
}

/// In-process client (+ manager, no network).
fn local_client() -> (ResourceClient, Arc<ResourceManager>) {
    let mgr = Arc::new(ResourceManager::new(policy()));
    let client = ResourceClient::new("agent1", Some(mgr.clone()), None);
    (client, mgr)
}

fn write_temp(name: &str, data: &[u8]) -> (std::path::PathBuf, String) {
    let path = std::env::temp_dir().join(name);
    let mut f = std::fs::File::create(&path).expect("create temp");
    f.write_all(data).unwrap();
    drop(f);
    let uri = format!("file://{}", path.display());
    (path, uri)
}

async fn read_all(mut s: ChunkStream) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(chunk) = s.next().await {
        out.extend(chunk.expect("chunk"));
    }
    out
}

// ---- shared behavior, run against both local and QUIC clients ----

/// Register the file locally when a manager is present (auto-bind only kicks
/// in server-side; the client's local-first routing requires registration).
async fn maybe_register(mgr: Option<&Arc<ResourceManager>>, path: &std::path::Path) {
    if let Some(m) = mgr {
        m.register(Box::new(FileResource::new(path)), "agent1".to_string())
            .await
            .unwrap();
    }
}

async fn read_end_to_end(client: &ResourceClient, mgr: Option<&Arc<ResourceManager>>) {
    let data: Vec<u8> = (0..1_000_000u64).map(|i| (i % 251) as u8).collect();
    let (path, uri) = write_temp("bos_stream_read_e2e", &data);
    maybe_register(mgr, &path).await;

    // Full read.
    let got = read_all(client.read_stream(&uri, 0, None).await.unwrap()).await;
    assert_eq!(got, data, "full stream must match source bytes");

    // Windowed read.
    let got = read_all(client.read_stream(&uri, 12345, Some(7777)).await.unwrap()).await;
    assert_eq!(got, data[12345..12345 + 7777]);

    // len past EOF clamps to available bytes.
    let got = read_all(
        client
            .read_stream(&uri, 999_000, Some(10_000))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(got, data[999_000..]);
}

async fn write_end_to_end(client: &ResourceClient, mgr: Option<&Arc<ResourceManager>>) {
    let (path, uri) = write_temp("bos_stream_write_e2e", b"");
    maybe_register(mgr, &path).await;
    let mut w = client.write_stream(&uri, 0).await.unwrap();
    let mut expected = Vec::new();
    for i in 0..100u32 {
        let chunk = format!("chunk-{i:04}-{}", "x".repeat(1_000)).into_bytes();
        w.write_chunk(&chunk).await.unwrap();
        expected.extend(chunk);
    }
    let n = ChunkWriter::finish(&mut *w).await.unwrap();
    assert_eq!(n as usize, expected.len());
    let on_disk = std::fs::read(path).unwrap();
    assert_eq!(on_disk, expected);
}

// ---- tests ----

#[tokio::test]
async fn local_stream_read_write() {
    let (client, mgr) = local_client();
    read_end_to_end(&client, Some(&mgr)).await;
    write_end_to_end(&client, Some(&mgr)).await;
}

#[tokio::test]
async fn quic_stream_read_write() {
    let mgr = Arc::new(ResourceManager::new(policy()));
    let client = connected_client(mgr).await;
    read_end_to_end(&client, None).await;
    write_end_to_end(&client, None).await;
}

/// Chunks must arrive *incrementally* over QUIC — the first chunk of a 1 MiB
/// read must arrive even while the read is still logically in flight. Guards
/// against a regression to the old buffered dispatch path.
#[tokio::test]
async fn quic_stream_is_incremental() {
    let data = vec![0x5Au8; 1_048_576]; // 1 MiB
    let (_path, uri) = write_temp("bos_stream_incremental", &data);

    let mgr = Arc::new(ResourceManager::new(policy()));
    let client = connected_client(mgr).await;

    let mut stream = client.read_stream(&uri, 0, None).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("first chunk")
        .expect("first chunk item")
        .expect("first chunk ok");
    assert!(!first.is_empty());
    assert!(
        first.len() < data.len(),
        "chunks must be smaller than the whole body"
    );
    // The rest must total the file.
    let mut rest = Vec::new();
    while let Some(c) = stream.next().await {
        rest.extend(c.expect("chunk"));
    }
    let total = first.len() + rest.len();
    assert_eq!(total, data.len());
}

/// Errors surface through the stream, not as a silent hang.
#[tokio::test]
async fn quic_stream_read_missing_file() {
    let mgr = Arc::new(ResourceManager::new(policy()));
    let client = connected_client(mgr).await;
    let uri = format!("file://{}/no-such-file-xyz", std::env::temp_dir().display());
    // Auto-bind creates a FileResource handler eagerly; the missing-path error
    // surfaces on the first streamed chunk.
    let stream = client.read_stream(&uri, 0, None).await;
    match stream {
        Err(_) => {}
        Ok(mut s) => {
            let item = s.next().await.expect("expected an error item");
            assert!(item.is_err(), "expected Err chunk, got {item:?}");
        }
    }
}
