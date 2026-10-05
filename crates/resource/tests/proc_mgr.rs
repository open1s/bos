//! Process management: spawn / kill / wait / list / exit-events, local and
//! over QUIC. The manager (`proc://`) registers each spawned child as
//! `proc://<pid>`, so the same address space works in-process and cross-node.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use rcgen::{BasicConstraints, CertificateParams, CertifiedKey, DnType, IsCa, KeyPair};
use resource::prelude::*;
use resource::transport::{QuicServer, QuicTransport};
use resource::{Effect, ProcManager, Rule};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

fn policy() -> SharedPolicy {
    SharedPolicy::new(PolicyDoc {
        admins: vec!["admin".to_string()],
        rules: vec![Rule {
            agents: vec!["agent1".to_string()],
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
        .expect("bind");
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let trust = ca_pem(&ca);
    let t = QuicTransport::new(
        local,
        "server".into(),
        &trust,
        cert_der(&client_k),
        key_der(&client_k),
    )
    .expect("client");
    ResourceClient::new("agent1", None, Some(Arc::new(t)))
}

/// Spawn `/bin/sh -c <script>` through the manager; returns the pid.
async fn spawn_shell(client: &ResourceClient, script: &str) -> u32 {
    let out = client
        .invoke(
            "proc://",
            ResourceAction::Spawn {
                args: vec!["/bin/sh".into(), "-c".into(), script.into()],
                env: vec![],
            },
        )
        .await
        .expect("spawn");
    match out {
        ResourceOutput::Spawned { pid } => pid,
        other => panic!("expected Spawned, got {other:?}"),
    }
}

fn proc_uri(pid: u32) -> String {
    format!("proc://{pid}")
}

async fn proc_suite(client: &ResourceClient) {
    // Spawn echo → exits 0 on its own; Wait observes the exit.
    let pid = spawn_shell(client, "echo hello && exit 0").await;
    assert!(pid > 0);
    let uri = proc_uri(pid);

    // Manager lists the child.
    let out = client
        .invoke("proc://", ResourceAction::List { pattern: None })
        .await
        .unwrap();
    match out {
        ResourceOutput::Listed { entries } => {
            assert!(
                entries.iter().any(|e| e.contains(&uri)),
                "list should contain {uri}: {entries:?}"
            );
        }
        other => panic!("expected Listed, got {other:?}"),
    }

    // Wait for exit.
    let out = client.invoke(&uri, ResourceAction::Wait).await.unwrap();
    assert!(
        matches!(out, ResourceOutput::Exited { code: 0 }),
        "echo should exit 0: {out:?}"
    );

    // env propagation: exit code carries it.
    let pid2 = spawn_shell(client, "exit 7").await;
    let uri2 = proc_uri(pid2);
    let out = client.invoke(&uri2, ResourceAction::Wait).await.unwrap();
    assert!(
        matches!(out, ResourceOutput::Exited { code: 7 }),
        "expected 7, got {out:?}"
    );

    // Kill a long-running process.
    let pid3 = spawn_shell(client, "exec sleep 300").await;
    let uri3 = proc_uri(pid3);
    client
        .invoke(&uri3, ResourceAction::Kill { signal: 9 })
        .await
        .unwrap();
    let out = client.invoke(&uri3, ResourceAction::Wait).await.unwrap();
    match out {
        ResourceOutput::Exited { code } => assert_ne!(code, 0, "killed process must not exit 0"),
        other => panic!("expected Exited after kill, got {other:?}"),
    }
}

/// Subscribe to a process and observe the `Exited` event.
async fn proc_events(client: &ResourceClient) {
    let pid = spawn_shell(client, "sleep 0.2; exit 42").await;
    let uri = proc_uri(pid);
    let mut sub = client
        .subscribe(&uri, vec![])
        .await
        .expect("subscribe proc");
    let got = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = sub.next().await {
            if let ResourceEvent::Exited(code) = ev {
                return code;
            }
        }
        -1
    })
    .await;
    assert_eq!(got, Ok(42), "expected Exited(42)");
}

#[tokio::test]
async fn local_proc_manager() {
    let mgr = Arc::new(ResourceManager::new(policy()));
    mgr.register(Box::new(ProcManager::new(mgr.clone())), "admin".to_string())
        .await
        .unwrap();
    let client = ResourceClient::new("agent1", Some(mgr.clone()), None);
    proc_suite(&client).await;
    proc_events(&client).await;
}

#[tokio::test]
async fn quic_proc_manager() {
    let mgr = Arc::new(ResourceManager::new(policy()));
    mgr.register(Box::new(ProcManager::new(mgr.clone())), "admin".to_string())
        .await
        .unwrap();
    let client = connected_client(mgr).await;
    proc_suite(&client).await;
    proc_events(&client).await;
}
