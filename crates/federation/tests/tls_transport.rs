//! A sealed frame crosses two real Zenoh sessions over TLS.

use std::net::TcpListener;
use std::time::Duration;

use bus::{Bus, BusConfig, TlsConfig};
use capability::transport::{unix_ms, FederationMessage, FederationTransport};
use capability::DeliveryState;
use federation::{Envelope, NodeIdentity, ZenohTransport};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback listener")
        .local_addr()
        .expect("an address")
        .port()
}

struct Material {
    ca: std::path::PathBuf,
    server_cert: std::path::PathBuf,
    server_key: std::path::PathBuf,
    client_cert: std::path::PathBuf,
    client_key: std::path::PathBuf,
    root: std::path::PathBuf,
}

fn write_material(label: &str) -> Material {
    let root = std::env::temp_dir().join(format!("bos-tls-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch");

    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "bsh federation test ca");
    let ca_key = KeyPair::generate().expect("ca key");
    let ca = ca_params.self_signed(&ca_key).expect("ca cert");

    let mut server_params =
        CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .expect("server params");
    server_params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    let server_key = KeyPair::generate().expect("server key");
    let server = server_params
        .signed_by(&server_key, &ca, &ca_key)
        .expect("server cert");

    let mut client_params = CertificateParams::new(Vec::<String>::new()).expect("client params");
    client_params
        .distinguished_name
        .push(DnType::CommonName, "bsh test client");
    let client_key = KeyPair::generate().expect("client key");
    let client = client_params
        .signed_by(&client_key, &ca, &ca_key)
        .expect("client cert");

    let ca_path = root.join("ca.pem");
    let server_cert_path = root.join("server.pem");
    let server_key_path = root.join("server.key");
    let client_cert_path = root.join("client.pem");
    let client_key_path = root.join("client.key");
    std::fs::write(&ca_path, ca.pem()).expect("ca pem");
    std::fs::write(&server_cert_path, server.pem()).expect("server pem");
    std::fs::write(&server_key_path, server_key.serialize_pem()).expect("server key");
    std::fs::write(&client_cert_path, client.pem()).expect("client pem");
    std::fs::write(&client_key_path, client_key.serialize_pem()).expect("client key");
    Material {
        ca: ca_path,
        server_cert: server_cert_path,
        server_key: server_key_path,
        client_cert: client_cert_path,
        client_key: client_key_path,
        root,
    }
}

fn tls(listen: Option<u16>, connect: Option<u16>, material: &Material) -> BusConfig {
    BusConfig {
        mode: "peer".to_string(),
        connect: connect.map(|port| vec![format!("tls/localhost:{port}")]),
        listen: listen.map(|port| vec![format!("tls/127.0.0.1:{port}")]),
        peer: None,
        scouting: false,
        tls: Some(TlsConfig {
            root_ca_certificate: Some(material.ca.display().to_string()),
            certificate: Some(if listen.is_some() {
                material.server_cert.display().to_string()
            } else {
                material.client_cert.display().to_string()
            }),
            private_key: Some(if listen.is_some() {
                material.server_key.display().to_string()
            } else {
                material.client_key.display().to_string()
            }),
            enable_mtls: Some(true),
            verify_name_on_connect: Some(true),
        }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sealed_frame_crosses_mutual_tls() {
    let material = write_material("single");
    let port = free_port();
    let listener = Bus::from(tls(Some(port), None, &material)).await;
    let connector = Bus::from(tls(None, Some(port), &material)).await;

    let alice = NodeIdentity::from_seed([13u8; 32]);
    let bob = NodeIdentity::from_seed([14u8; 32]);
    let alice_transport = ZenohTransport::new(connector.session(), alice.node_id());
    let bob_transport = ZenohTransport::new(listener.session(), bob.node_id());

    let mut inbox = bob_transport.subscribe().await.expect("an inbox");
    tokio::time::sleep(Duration::from_millis(1_500)).await;

    let message = FederationMessage::new(
        "invoke",
        br#"{"capability":"builtin/echo_add","input":{"a":1,"b":2}}"#.to_vec(),
    );
    let envelope = Envelope::seal(&message, &alice, &bob.node_id()).expect("sealed");
    let frame = envelope.encode().expect("encoded");

    let state = alice_transport
        .send(&bob.node_id(), frame)
        .await
        .expect("the frame leaves alice over TLS");
    assert_eq!(state, DeliveryState::Sent);

    let received = inbox
        .recv_timeout(Duration::from_secs(10))
        .await
        .expect("the frame arrives at bob over TLS");
    let decoded = Envelope::decode(&received).expect("decoded");
    assert_eq!(decoded.sender, alice.node_id());
    assert_eq!(decoded.recipient, bob.node_id());
    decoded.open(unix_ms()).expect("verified and fresh");

    let _ = std::fs::remove_dir_all(material.root);
}

fn child_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Child half of the cross-process TLS test: opens a TLS listener, waits for
/// one sealed frame, and writes it to the path the parent named.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tls_child_listener() {
    let Some(root) = child_env("BSH_TLS_ROOT") else {
        return;
    };
    let port: u16 = child_env("BSH_TLS_PORT")
        .expect("port")
        .parse()
        .expect("port");
    let output = child_env("BSH_TLS_OUTPUT").expect("output");
    let ready = child_env("BSH_TLS_READY").expect("ready");
    let material = Material {
        ca: std::path::PathBuf::from(&root).join("ca.pem"),
        server_cert: std::path::PathBuf::from(&root).join("server.pem"),
        server_key: std::path::PathBuf::from(&root).join("server.key"),
        client_cert: std::path::PathBuf::from(&root).join("client.pem"),
        client_key: std::path::PathBuf::from(&root).join("client.key"),
        root: std::path::PathBuf::from(&root),
    };
    let listener = Bus::from(tls(Some(port), None, &material)).await;
    let identity = NodeIdentity::from_seed([24u8; 32]);
    let transport = ZenohTransport::new(listener.session(), identity.node_id());
    let mut inbox = transport.subscribe().await.expect("an inbox");
    std::fs::write(&ready, b"ready").expect("ready file");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let frame = inbox
        .recv_timeout(Duration::from_secs(15))
        .await
        .expect("the frame arrives in the child");
    std::fs::write(&output, frame).expect("output");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sealed_frame_crosses_mutual_tls_between_processes() {
    let material = write_material("rogue");
    let port = free_port();
    let ready = material.root.join("ready");
    let output = material.root.join("frame");
    let exe = std::env::current_exe().expect("the test executable");
    let mut child = std::process::Command::new(exe)
        .arg("--exact")
        .arg("tls_child_listener")
        .arg("--nocapture")
        .env("BSH_TLS_ROOT", &material.root)
        .env("BSH_TLS_PORT", port.to_string())
        .env("BSH_TLS_OUTPUT", &output)
        .env("BSH_TLS_READY", &ready)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the child test starts");
    for _ in 0..150 {
        if ready.is_file() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready.is_file(), "the TLS child became ready");

    let connector = Bus::from(tls(None, Some(port), &material)).await;
    let alice = NodeIdentity::from_seed([23u8; 32]);
    let bob = NodeIdentity::from_seed([24u8; 32]);
    let alice_transport = ZenohTransport::new(connector.session(), alice.node_id());
    let message = FederationMessage::new(
        "invoke",
        br#"{"capability":"builtin/echo_add","input":{"a":1,"b":2}}"#.to_vec(),
    );
    let envelope = Envelope::seal(&message, &alice, &bob.node_id()).expect("sealed");
    let frame = envelope.encode().expect("encoded");
    tokio::time::sleep(Duration::from_millis(500)).await;
    alice_transport
        .send(&bob.node_id(), frame)
        .await
        .expect("the frame leaves alice over TLS");

    for _ in 0..150 {
        if output.is_file() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let received = std::fs::read(&output).expect("the child wrote the frame");
    let decoded = Envelope::decode(&received).expect("decoded");
    assert_eq!(decoded.sender, alice.node_id());
    assert_eq!(decoded.recipient, bob.node_id());
    decoded.open(unix_ms()).expect("verified and fresh");

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(material.root);
}

fn write_rogue_client(material: &Material) -> (std::path::PathBuf, std::path::PathBuf) {
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("rogue ca");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "untrusted ca");
    let ca_key = KeyPair::generate().expect("rogue ca key");
    let ca = ca_params.self_signed(&ca_key).expect("rogue ca cert");

    let mut params = CertificateParams::new(Vec::<String>::new()).expect("rogue params");
    params
        .distinguished_name
        .push(DnType::CommonName, "rogue client");
    let key = KeyPair::generate().expect("rogue key");
    let cert = params.signed_by(&key, &ca, &ca_key).expect("rogue cert");
    let cert_path = material.root.join("rogue-client.pem");
    let key_path = material.root.join("rogue-client.key");
    std::fs::write(&cert_path, cert.pem()).expect("rogue cert pem");
    std::fs::write(&key_path, key.serialize_pem()).expect("rogue key pem");
    (cert_path, key_path)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_certificate_from_another_ca_is_refused() {
    let material = write_material("process");
    let (rogue_cert, rogue_key) = write_rogue_client(&material);
    let port = free_port();
    let listener = Bus::from(tls(Some(port), None, &material)).await;
    let bob = NodeIdentity::from_seed([44u8; 32]);
    let bob_transport = ZenohTransport::new(listener.session(), bob.node_id());
    let mut inbox = bob_transport.subscribe().await.expect("inbox");

    let config = BusConfig {
        mode: "peer".to_string(),
        connect: Some(vec![format!("tls/localhost:{port}")]),
        listen: None,
        peer: None,
        scouting: false,
        tls: Some(TlsConfig {
            root_ca_certificate: Some(material.ca.display().to_string()),
            certificate: Some(rogue_cert.display().to_string()),
            private_key: Some(rogue_key.display().to_string()),
            enable_mtls: Some(true),
            verify_name_on_connect: Some(true),
        }),
    };
    let alice = NodeIdentity::from_seed([45u8; 32]);
    let connector = tokio::time::timeout(Duration::from_secs(5), Bus::from(config)).await;
    if let Ok(connector) = connector {
        let alice_transport = ZenohTransport::new(connector.session(), alice.node_id());
        let message =
            FederationMessage::new("invoke", br#"{"capability":"builtin/echo"}"#.to_vec());
        let envelope = Envelope::seal(&message, &alice, &bob.node_id()).expect("sealed");
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            alice_transport.send(&bob.node_id(), envelope.encode().expect("encoded")),
        )
        .await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        inbox
            .recv_timeout(Duration::from_millis(500))
            .await
            .is_none(),
        "the listener must not accept a frame from a client signed by another CA"
    );
    let _ = std::fs::remove_dir_all(material.root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tls_session_reconnects_after_the_listener_restarts() {
    let material = write_material("restart");
    let port = free_port();
    let listener = Bus::from(tls(Some(port), None, &material)).await;
    let connector = Bus::from(tls(None, Some(port), &material)).await;
    let alice = NodeIdentity::from_seed([33u8; 32]);
    let bob = NodeIdentity::from_seed([34u8; 32]);
    let alice_transport = ZenohTransport::new(connector.session(), alice.node_id());
    let bob_transport = ZenohTransport::new(listener.session(), bob.node_id());
    let mut inbox = bob_transport.subscribe().await.expect("inbox");
    tokio::time::sleep(Duration::from_millis(750)).await;

    let first = Envelope::seal(
        &FederationMessage::new("invoke", br#"{"n":1}"#.to_vec()),
        &alice,
        &bob.node_id(),
    )
    .expect("sealed")
    .encode()
    .expect("encoded");
    alice_transport
        .send(&bob.node_id(), first)
        .await
        .expect("first frame");
    assert!(inbox.recv_timeout(Duration::from_secs(5)).await.is_some());

    drop(inbox);
    drop(bob_transport);
    drop(listener);
    let listener = Bus::from(tls(Some(port), None, &material)).await;
    let bob_transport = ZenohTransport::new(listener.session(), bob.node_id());
    let mut inbox = bob_transport.subscribe().await.expect("inbox");
    tokio::time::sleep(Duration::from_millis(1_500)).await;

    let second = Envelope::seal(
        &FederationMessage::new("invoke", br#"{"n":2}"#.to_vec()),
        &alice,
        &bob.node_id(),
    )
    .expect("sealed")
    .encode()
    .expect("encoded");
    alice_transport
        .send(&bob.node_id(), second)
        .await
        .expect("second frame");
    let received = inbox
        .recv_timeout(Duration::from_secs(10))
        .await
        .expect("the connector reconnects to the restarted TLS listener");
    Envelope::decode(&received).expect("decoded");
    let _ = std::fs::remove_dir_all(material.root);
}
