//! vnode sub-path traversal over QUIC: a client resolves and lists
//! `vnode://<name>/<sub>` where the base vnode aggregates a remote folder.
//! Reproduces `rex tree vnode://nodeA/certs` that previously failed to resolve.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rcgen::{BasicConstraints, CertificateParams, CertifiedKey, DnType, IsCa, KeyPair};
use resource::prelude::*;
use resource::transport::{QuicServer, QuicTransport, Transport};
use resource::{Effect, Rule};
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
    let mut p = CertificateParams::new(vec!["BOS-CA".to_string()]).unwrap();
    p.distinguished_name.push(DnType::CommonName, "BOS-CA");
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let kp = KeyPair::generate().unwrap();
    CertifiedKey {
        cert: p.self_signed(&kp).unwrap(),
        key_pair: kp,
    }
}

fn gen_entity(ca: &CertifiedKey, cn: &str) -> CertifiedKey {
    let kp = KeyPair::generate().unwrap();
    let mut p = CertificateParams::new(vec![cn.to_string()]).unwrap();
    p.distinguished_name.push(DnType::CommonName, cn);
    CertifiedKey {
        cert: p.signed_by(&kp, &ca.cert, &ca.key_pair).unwrap(),
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

#[tokio::test]
async fn vnode_subpath_over_quic() {
    // A remote folder (node "A") with nested content.
    let root = std::env::temp_dir().join("bos_vq_a");
    std::fs::create_dir_all(root.join("certs")).unwrap();
    std::fs::write(root.join("certs").join("ca.pem"), b"CA-DATA").unwrap();

    let ca = gen_ca();
    let server_k = gen_entity(&ca, "server");
    let client_k = gen_entity(&ca, "me");

    // Server "B" hosts a vnode aggregating A's folder over its own QUIC
    // transport to A. In rex this exact wiring is what discovery builds.
    let a_mgr = Arc::new(ResourceManager::new(policy()));
    let _ = a_mgr; // A's folder auto-binds server-side on the B→A transport.

    let a_server = Arc::new(QuicServer::new(a_mgr.clone()));
    let a_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let a_local = a_server
        .bind(
            a_addr,
            cert_der(&server_k),
            key_der(&server_k),
            &ca_pem(&ca),
            &[],
        )
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = a_server.run().await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let transport_to_a: Arc<dyn Transport> = Arc::new(
        QuicTransport::new(
            a_local,
            "server".into(),
            &ca_pem(&ca),
            cert_der(&client_k),
            key_der(&client_k),
        )
        .unwrap(),
    );

    // B's manager holds the vnode.
    let b_mgr = Arc::new(ResourceManager::new(policy()));
    let vnode = VirtualNodeResource::new(
        "nodeA",
        "me",
        vec![(format!("folder://{}", root.display()), transport_to_a)],
    );
    b_mgr
        .register(Box::new(vnode), "admin".into())
        .await
        .unwrap();

    // B serves QUIC; a client talks to B.
    let b_server = Arc::new(QuicServer::new(b_mgr));
    let b_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let b_local = b_server
        .bind(
            b_addr,
            cert_der(&server_k),
            key_der(&server_k),
            &ca_pem(&ca),
            &[],
        )
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = b_server.run().await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let client_transport: Arc<dyn Transport> = Arc::new(
        QuicTransport::new(
            b_local,
            "server".into(),
            &ca_pem(&ca),
            cert_der(&client_k),
            key_der(&client_k),
        )
        .unwrap(),
    );
    let client = ResourceClient::new("me", None, Some(client_transport));

    // 1. Base resolve: vnode://nodeA exists.
    assert!(client.resolve("vnode://nodeA").await.unwrap().is_some());

    // 2. Sub-path resolve: vnode://nodeA/certs (the previously-failing call).
    let sub = client.resolve("vnode://nodeA/certs").await.unwrap();
    assert!(sub.is_some(), "vnode sub-path must resolve");

    // 3. List the sub-path through the vnode → union of certs/ contents.
    let out = client
        .invoke(
            "vnode://nodeA/certs",
            ResourceAction::List { pattern: None },
        )
        .await
        .unwrap();
    match out {
        ResourceOutput::Listed { entries } => assert_eq!(entries, vec!["ca.pem"]),
        other => panic!("expected Listed, got {other:?}"),
    }

    // 4. Read a file two levels deep (the exact `tree vnode://nodeA/certs/ca.pem`
    //    leaf the explorer would open).
    client
        .invoke("vnode://nodeA/certs/ca.pem", ResourceAction::Open)
        .await
        .unwrap();
}
