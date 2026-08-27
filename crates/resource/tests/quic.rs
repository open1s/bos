//! End-to-end test of the QUIC transport: a real mTLS connection between a
//! `QuicTransport` client and a `QuicServer` backed by a `ResourceManager`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rcgen::{BasicConstraints, CertificateParams, CertifiedKey, DnType, IsCa, KeyPair};
use resource::action::ResourceAction;
use resource::policy::{Effect, PolicyDoc, Rule, SharedPolicy};
use resource::prelude::*;
use resource::transport::{QuicServer, QuicTransport};
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
    CertifiedKey {
        cert,
        key_pair: kp,
    }
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

/// Spin up a mTLS QUIC server backed by `mgr` and return a connected client.
async fn connected_client(mgr: Arc<ResourceManager>) -> ResourceClient {
    let ca = gen_ca();
    let server_k = gen_entity(&ca, "server");
    let client_k = gen_entity(&ca, "agent1");

    let server = Arc::new(QuicServer::new(mgr));
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let local = server
        .bind(addr, cert_der(&server_k), key_der(&server_k), &ca_pem(&ca), &[])
        .await
        .expect("bind server");
    let srv = server.clone();
    tokio::spawn(async move {
        let _ = srv.run().await;
    });

    // Give the server a moment to start listening.
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

#[tokio::test]
async fn quic_file_roundtrip() {
    let path = std::env::temp_dir().join("resource_test_quic");
    let _ = std::fs::remove_file(&path);
    let uri = format!("file://{}", path.display());

    let mgr = Arc::new(ResourceManager::new(policy()));
    mgr.register(Box::new(FileResource::new(&path)), "agent1".to_string())
        .await
        .unwrap();
    let client = connected_client(mgr).await;

    assert!(matches!(
        client.invoke(&uri, ResourceAction::Open).await,
        Ok(ResourceOutput::Opened)
    ));
    assert!(matches!(
        client
            .invoke(&uri, ResourceAction::Write { offset: 0, data: b"quic!".to_vec() })
            .await,
        Ok(ResourceOutput::WriteOk { written: 5 })
    ));
    let out = client
        .invoke(&uri, ResourceAction::Read { offset: 0, len: 5 })
        .await
        .unwrap();
    match out {
        ResourceOutput::ReadOk { data } => assert_eq!(data, b"quic!"),
        _ => panic!("expected ReadOk"),
    }

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn quic_large_read_streams() {
    let path = std::env::temp_dir().join("resource_test_quic_large");
    let _ = std::fs::remove_file(&path);
    let uri = format!("file://{}", path.display());

    let mgr = Arc::new(ResourceManager::new(policy()));
    mgr.register(Box::new(FileResource::new(&path)), "agent1".to_string())
        .await
        .unwrap();
    let client = connected_client(mgr).await;

    let big = vec![0xABu8; 256 * 1024]; // 256 KiB > STREAM_THRESHOLD (64 KiB)
    client.invoke(&uri, ResourceAction::Open).await.unwrap();
    client
        .invoke(&uri, ResourceAction::Write { offset: 0, data: big.clone() })
        .await
        .unwrap();
    let out = client
        .invoke(&uri, ResourceAction::Read { offset: 0, len: big.len() as u64 })
        .await
        .unwrap();
    match out {
        ResourceOutput::ReadOk { data } => assert_eq!(data, big),
        _ => panic!("expected ReadOk from streamed body"),
    }
    let _ = std::fs::remove_file(&path);
}
