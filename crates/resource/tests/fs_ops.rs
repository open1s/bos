//! Full-POSIX fs ops across local and QUIC paths: truncate, rename, stat,
//! mkdir, remove, lock/unlock, and filesystem watch (Subscribe on file/folder).

use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use rcgen::{BasicConstraints, CertificateParams, CertifiedKey, DnType, IsCa, KeyPair};
use resource::prelude::*;
use resource::transport::{QuicServer, QuicTransport};
use resource::{PolicyDoc, Rule, Effect};
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
        .bind(addr, cert_der(&server_k), key_der(&server_k), &ca_pem(&ca), &[])
        .await
        .expect("bind server");
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let trust = ca_pem(&ca);
    let t = QuicTransport::new(local, "server".to_string(), &trust, cert_der(&client_k), key_der(&client_k))
        .expect("client");
    ResourceClient::new("agent1", None, Some(Arc::new(t)))
}

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bos_fsops_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_file(path: &std::path::Path, data: &[u8]) {
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(data).unwrap();
}

async fn stat_ok(client: &ResourceClient, uri: &str) -> (u64, bool, bool, i64) {
    match client.invoke(uri, ResourceAction::Stat).await.unwrap() {
        ResourceOutput::StatOk { size, is_dir, readonly, modified_secs } => {
            (size, is_dir, readonly, modified_secs)
        }
        other => panic!("expected StatOk, got {other:?}"),
    }
}

/// Truncate + Stat + Rename + Lock + Unlock + Remove, all on `file://`.
async fn file_ops(client: &ResourceClient, mgr: Option<&Arc<ResourceManager>>) {
    let dir = tmpdir("file");
    let path = dir.join("data.txt");
    write_file(&path, b"hello world 0123456789");
    let uri = format!("file://{}", path.display());
    if let Some(m) = mgr {
        m.register(Box::new(FileResource::new(&path)), "agent1".into())
            .await
            .unwrap();
    }

    // Stat on the existing file.
    let (size, is_dir, _, mtime) = stat_ok(client, &uri).await;
    assert_eq!(size, 22);
    assert!(!is_dir);
    assert!(mtime > 0);

    // Truncate to 5 bytes.
    assert!(matches!(
        client.invoke(&uri, ResourceAction::Truncate { len: 5 }).await,
        Ok(ResourceOutput::Truncated)
    ));
    let (size, ..) = stat_ok(client, &uri).await;
    assert_eq!(size, 5);

    // Extend via truncate — new tail is zero-filled.
    client.invoke(&uri, ResourceAction::Truncate { len: 8 }).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"hello\0\0\0");

    // Rename. The registry still keys the handler by the old URI (routing is
    // unchanged); the handler's on-disk path moves. We additionally register
    // the new path so the new URI resolves.
    let new_path = dir.join("moved.txt");
    let new_uri = format!("file://{}", new_path.display());
    let r = client
        .invoke(&uri, ResourceAction::Rename { new_uri: new_uri.clone() })
        .await;
    assert!(matches!(r, Ok(ResourceOutput::Renamed)), "rename failed: {r:?}");
    assert!(new_path.exists() && !path.exists());
    if let Some(m) = mgr {
        m.register(Box::new(FileResource::new(&new_path)), "agent1".into())
            .await
            .unwrap();
    }
    let (size, ..) = stat_ok(client, &new_uri).await;
    assert_eq!(size, 8);

    // Lock exclusivity: second exclusive lock fails, unlock releases.
    assert!(matches!(
        client.invoke(&new_uri, ResourceAction::Lock { exclusive: true }).await,
        Ok(ResourceOutput::Locked)
    ));
    let conflict = client
        .invoke(&new_uri, ResourceAction::Lock { exclusive: true })
        .await;
    assert!(matches!(conflict, Err(ResourceError::Other(_)) | Err(ResourceError::Locked(_))));
    assert!(matches!(
        client.invoke(&new_uri, ResourceAction::Unlock).await,
        Ok(ResourceOutput::Unlocked)
    ));
    assert!(matches!(
        client.invoke(&new_uri, ResourceAction::Lock { exclusive: false }).await,
        Ok(ResourceOutput::Locked)
    ));

    // Remove.
    client.invoke(&new_uri, ResourceAction::Remove { recursive: false }).await.unwrap();
    assert!(!new_path.exists());
    client.invoke(&new_uri, ResourceAction::Stat).await.unwrap_err();
}

/// MkDir / List / Stat / Rename on `folder://`.
async fn folder_ops(client: &ResourceClient, mgr: Option<&Arc<ResourceManager>>) {
    let dir = tmpdir("folder");
    let root_uri = format!("folder://{}", dir.display());
    if let Some(m) = mgr {
        m.register(Box::new(FolderResource::new(&dir)), "agent1".into())
            .await
            .unwrap();
    }
    let sub = dir.join("sub");
    let sub_uri = format!("folder://{}", sub.display());
    // The dispatcher's auto-bind requires the path to exist, so for creation
    // we register the (not-yet-on-disk) folder handler up-front.
    if let Some(m) = mgr {
        m.register(Box::new(FolderResource::new(&sub)), "agent1".into())
            .await
            .unwrap();
    }

    client
        .invoke(&sub_uri, ResourceAction::MkDir { recursive: false })
        .await
        .unwrap();
    let (_, is_dir, ..) = stat_ok(client, &sub_uri).await;
    assert!(is_dir);

    write_file(&dir.join("alpha.txt"), b"a");
    write_file(&dir.join("beta.txt"), b"b");
    match client.invoke(&root_uri, ResourceAction::List { pattern: None }).await.unwrap() {
        ResourceOutput::Listed { entries } => {
            let mut e = entries;
            e.sort();
            assert_eq!(e, vec!["alpha.txt", "beta.txt", "sub"]);
        }
        other => panic!("expected Listed, got {other:?}"),
    }

    // Rename a directory.
    let sub2 = dir.join("sub2");
    let sub2_uri = format!("folder://{}", sub2.display());
    client
        .invoke(&sub_uri, ResourceAction::Rename { new_uri: sub2_uri.clone() })
        .await
        .unwrap();
    assert!(sub2.exists() && !sub.exists());
}

/// Watch a folder and observe a create event.
async fn watch_ops(client: &ResourceClient, mgr: Option<&Arc<ResourceManager>>) {
    let dir = tmpdir("watch");
    let uri = format!("folder://{}", dir.display());
    if let Some(m) = mgr {
        m.register(Box::new(FolderResource::new(&dir)), "agent1".into())
            .await
            .unwrap();
    }
    client
        .invoke(&uri, ResourceAction::Open) // ensure bound
        .await
        .unwrap();
    let mut sub = client.subscribe(&uri, vec!["create".to_string()]).await.unwrap();

    // Overlap: give the watcher a moment to register, then create a file.
    tokio::time::sleep(Duration::from_millis(400)).await;
    write_file(&dir.join("watched.txt"), b"hi");

    let got = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = sub.next().await {
            if let ResourceEvent::Custom(tag, payload) = ev {
                if tag == "create" && String::from_utf8_lossy(&payload).contains("watched.txt") {
                    return true;
                }
            }
        }
        false
    })
    .await;
    assert_eq!(got, Ok(true), "expected a create event for watched.txt");
}

#[tokio::test]
async fn local_file_ops() {
    let mgr = Arc::new(ResourceManager::new(policy()));
    let client = ResourceClient::new("agent1", Some(mgr.clone()), None);
    file_ops(&client, Some(&mgr)).await;
    folder_ops(&client, Some(&mgr)).await;
    watch_ops(&client, Some(&mgr)).await;
}

#[tokio::test]
async fn quic_file_ops() {
    let mgr = Arc::new(ResourceManager::new(policy()));
    let client = connected_client(mgr.clone()).await;
    file_ops(&client, Some(&mgr)).await;
    folder_ops(&client, Some(&mgr)).await;
    watch_ops(&client, Some(&mgr)).await;
}
