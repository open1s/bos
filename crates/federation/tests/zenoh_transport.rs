//! A sealed frame crosses two real Zenoh sessions on loopback.

use std::net::TcpListener;
use std::time::Duration;

use bus::{Bus, BusConfig};
use capability::transport::{unix_ms, FederationMessage, FederationTransport};
use capability::DeliveryState;
use federation::{inbox_key, node_key, Envelope, NodeIdentity, ZenohTransport};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback listener")
        .local_addr()
        .expect("an address")
        .port()
}

fn peer_config(listen: Option<u16>, connect: Option<u16>) -> BusConfig {
    BusConfig {
        mode: "peer".to_string(),
        connect: connect.map(|port| vec![format!("tcp/127.0.0.1:{port}")]),
        listen: listen.map(|port| vec![format!("tcp/127.0.0.1:{port}")]),
        peer: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sealed_frame_crosses_two_zenoh_sessions() {
    let port = free_port();
    let listener = Bus::from(peer_config(Some(port), None)).await;
    let connector = Bus::from(peer_config(None, Some(port))).await;

    let alice = NodeIdentity::from_seed([3u8; 32]);
    let bob = NodeIdentity::from_seed([4u8; 32]);
    let alice_transport = ZenohTransport::new(listener.session(), alice.node_id());
    let bob_transport = ZenohTransport::new(connector.session(), bob.node_id());

    let mut inbox = bob_transport.subscribe().await.expect("an inbox");
    tokio::time::sleep(Duration::from_millis(750)).await;

    let message = FederationMessage::new(
        "invoke",
        br#"{"capability":"builtin/echo_add","input":{"a":1,"b":2}}"#.to_vec(),
    );
    let envelope = Envelope::seal(&message, &alice, &bob.node_id()).expect("sealed");
    let frame = envelope.encode().expect("encoded");

    let state = alice_transport
        .send(&bob.node_id(), frame)
        .await
        .expect("the frame leaves alice");
    assert_eq!(state, DeliveryState::Sent);

    let received = inbox
        .recv_timeout(Duration::from_secs(10))
        .await
        .expect("the frame arrives at bob");
    let decoded = Envelope::decode(&received).expect("decoded");
    assert_eq!(decoded.sender, alice.node_id());
    assert_eq!(decoded.recipient, bob.node_id());
    decoded.open(unix_ms()).expect("verified and fresh");
}

#[test]
fn a_node_key_is_safe_for_a_key_expression() {
    let identity = NodeIdentity::from_seed([5u8; 32]);
    let node = identity.node_id();
    let key = node_key(&node).expect("a node key");

    assert_eq!(key.len(), 64);
    assert!(key.chars().all(|character| character.is_ascii_hexdigit()));
    assert_eq!(
        inbox_key(&node).expect("an inbox key"),
        format!("bsh/v1/node/{key}/inbox")
    );
    assert!(!inbox_key(&node).expect("an inbox key").contains('+'));
    assert!(!inbox_key(&node).expect("an inbox key").contains('='));
}

#[test]
fn a_node_id_that_is_not_a_key_is_refused() {
    let node = capability::NodeId::new("not-a-key").expect("a node id");
    assert!(node_key(&node).is_err());
}
