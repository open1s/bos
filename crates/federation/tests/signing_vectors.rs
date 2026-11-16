//! Canonical signing vectors: the fixed bytes every implementation must sign.

use capability::transport::MessageId;
use federation::{Envelope, FederationMessage, NodeIdentity};

const CANONICAL: &str = "{\"protocol\":\"bsh.fed/1\",\"schema_version\":1,\"message_id\":\"msg-vector-1\",\"kind\":\"invoke\",\"sender\":\"6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw=\",\"recipient\":\"/RckOFqgx1tk+3jNYC+h2ZH96/drE8WO1wLqyDXp9hg=\",\"issued_at_ms\":1700000000000,\"expires_at_ms\":1700000060000,\"correlation\":\"corr-1\",\"idempotency_key\":null,\"content_type\":\"application/json\",\"payload\":{\"capability\":\"builtin/read_file\",\"input\":{\"path\":\"a.txt\"}},\"signature\":\"\"}";
const SIGNATURE: &str =
    "FdehIT5DHeNGNx+NbDvQh7l+LGXAjFjcmkUFrzu9fSjQX66Qh83raQl+Gg8dBmvgCPD7Z6Iu6ttF/3EWZnjDCw==";

fn vector(payload: &[u8]) -> Envelope {
    let sender = NodeIdentity::from_seed([7u8; 32]);
    let recipient = NodeIdentity::from_seed([9u8; 32]).node_id();
    let mut message = FederationMessage::new("invoke", payload.to_vec());
    message.id = MessageId::new("msg-vector-1");
    let message = message.with_correlation("corr-1");
    Envelope::seal_at(&message, &sender, &recipient, 1_700_000_000_000).expect("sealed")
}

#[test]
fn the_canonical_form_and_signature_are_pinned() {
    let payload = br#"{"capability":"builtin/read_file","input":{"path":"a.txt"}}"#;
    let envelope = vector(payload);

    assert_eq!(
        String::from_utf8_lossy(&envelope.canonical_bytes()),
        CANONICAL,
        "the signed bytes are the specification"
    );
    assert_eq!(envelope.signature, SIGNATURE, "Ed25519 is deterministic");
    assert!(envelope.verify());
}

#[test]
fn payload_key_order_does_not_change_the_signature() {
    let forward = br#"{"capability":"builtin/read_file","input":{"path":"a.txt"}}"#;
    let reversed = br#"{"input":{"path":"a.txt"},"capability":"builtin/read_file"}"#;

    assert_eq!(
        vector(forward).signature,
        vector(reversed).signature,
        "object keys are sorted before signing"
    );
}

#[test]
fn a_changed_payload_or_recipient_fails_verification() {
    let mut envelope = vector(br#"{"capability":"builtin/read_file"}"#);
    envelope.payload = serde_json::json!({ "capability": "builtin/write_file" });
    assert!(!envelope.verify(), "the payload is signed");

    let mut envelope = vector(br#"{"capability":"builtin/read_file"}"#);
    envelope.recipient = NodeIdentity::from_seed([11u8; 32]).node_id();
    assert!(!envelope.verify(), "the recipient is signed");
}
