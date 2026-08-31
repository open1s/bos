//! Multi-hop relay: A reaches C via B. B is a ResourceManager with a
//! RelayResource pointing at C; A holds a RelayTransport over in-process B.
//!
//! ```text
//! A  --(in-process)-->  B  --(QUIC)-->  C
//! relay transport      relay://c       proc://, file://…
//! ```

use std::io::Write;
use std::net::SocketAddr;
use std::println;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use rcgen::{BasicConstraints, CertificateParams, CertifiedKey, DnType, IsCa, KeyPair};
use resource::prelude::*;
use resource::transport::inprocess::InProcessTransport;
use resource::transport::{QuicServer, QuicTransport};
use resource::{Effect, ProcManager, RelayResource, RelayTransport, Rule};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

fn policy() -> SharedPolicy {
    SharedPolicy::new(PolicyDoc {
        admins: vec!["admin".to_string()],
        rules: vec![Rule {
            agents: vec!["*".to_string()],
            uris: vec!["*".to_string()],
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

/// Spin up C: QUIC server on top of its manager, plus the transport to reach it.
async fn node_c() -> (Arc<ResourceManager>, Arc<dyn resource::Transport>) {
    let mgr = Arc::new(ResourceManager::new(policy()));
    mgr.register(
        Box::new(ProcManager::new(mgr.clone())),
        "admin".into(),
    )
    .await
    .unwrap();

    let ca = gen_ca();
    let server_k = gen_entity(&ca, "c-server");
    let client_k = gen_entity(&ca, "agent1");
    let server = Arc::new(QuicServer::new(mgr.clone()));
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let local = server
        .bind(addr, cert_der(&server_k), key_der(&server_k), &ca_pem(&ca), &[])
        .await
        .expect("bind C");
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let trust = ca_pem(&ca);
    let t = QuicTransport::new(local, "c-server".into(), &trust, cert_der(&client_k), key_der(&client_k))
        .expect("client B→C");
    (mgr, Arc::new(t))
}

/// Assemble A → B → C.
async fn three_nodes() -> ResourceClient {
    let (_c_mgr, c_transport) = node_c().await;

    // B: intermediate manager with relay to C.
    let b = Arc::new(ResourceManager::new(policy()));
    b.register(Box::new(RelayResource::new("c", c_transport)), "admin".into())
        .await
        .unwrap();

    // A: relay transport over in-process B, then a ResourceClient with the relay.
    let upstream: Arc<dyn resource::Transport> = Arc::new(InProcessTransport::new(b, "admin"));
    let relay = RelayTransport::new(upstream, "c");
    ResourceClient::new("agent1", None, Some(Arc::new(relay)))
}

#[tokio::test]
async fn relay_spawn_kill_wait() {
    let client = three_nodes().await;

    // Spawn through the relay.
    let out = client
        .invoke(
            "proc://",
            ResourceAction::Spawn {
                args: vec!["/bin/sh".into(), "-c".into(), "exit 3".into()],
                env: vec![],
            },
        )
        .await
        .unwrap();
    let pid = match out {
        ResourceOutput::Spawned { pid } => pid,
        other => panic!("spawn through relay: {other:?}"),
    };

    // Wait through the relay — sees the exit propagated from C.
    let uri = format!("proc://{pid}");
    let out = client.invoke(&uri, ResourceAction::Wait).await.unwrap();
    assert!(matches!(out, ResourceOutput::Exited { code: 3 }), "got {out:?}");

    // Kill through the relay.
    let out = client
        .invoke(
            "proc://",
            ResourceAction::Spawn {
                args: vec!["/bin/sh".into(), "-c".into(), "exec sleep 60".into()],
                env: vec![],
            },
        )
        .await
        .unwrap();
    let pid = match out {
        ResourceOutput::Spawned { pid } => pid,
        _ => panic!("spawn sleep"),
    };
    let uri = format!("proc://{pid}");
    client
        .invoke(&uri, ResourceAction::Kill { signal: 9 })
        .await
        .unwrap();
    let out = client.invoke(&uri, ResourceAction::Wait).await.unwrap();
    assert!(matches!(out, ResourceOutput::Exited { .. }));
}

#[tokio::test]
async fn relay_file_roundtrip() {
    let client = three_nodes().await;
    let path = std::env::temp_dir().join("bos_relay_rw");
    let _ = std::fs::remove_file(&path);
    let uri = format!("file://{}", path.display());

    // Seed the file, then Open it through the relay (Write/Read need an open
    // handle; the relay streams those as buffered invoke actions).
    std::fs::File::create(&path).unwrap().write_all(b"seed").unwrap();
    client.invoke(&uri, ResourceAction::Open).await.unwrap();

    let mut w = client.write_stream(&uri, 4).await.unwrap();
    w.write_chunk(b"hello").await.unwrap();
    let n = resource::ChunkWriter::finish(&mut *w).await.unwrap();
    assert_eq!(n, 5);
    assert_eq!(std::fs::read(&path).unwrap(), b"seedhello");

    // Read back through the relay's chunked read_stream.
    let mut s = client.read_stream(&uri, 0, None).await.unwrap();
    let mut data = Vec::new();
    while let Some(chunk) = s.next().await {
        data.extend(chunk.unwrap());
    }
    assert_eq!(data, b"seedhello");
    println!("relay file roundtrip: read {data:?}");
    let _ = std::fs::remove_file(&path);
}
