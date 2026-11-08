//! The signed, versioned JSON envelope every federation message travels in.
//!
//! The format is JSON on purpose (ADR-009): two BSH builds must be able to read
//! each other's messages, and the bus codec (rkyv) is build-specific. The
//! signature covers a canonical serialization with the signature field empty,
//! so a relay cannot alter a field without invalidating the message.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use capability::{unix_ms, FederationMessage, NodeId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::identity::{self, NodeIdentity};

/// The protocol name and version this build speaks.
pub const PROTOCOL: &str = "bsh.fed/1";

/// The protocol major this build speaks.
pub const PROTOCOL_MAJOR: u32 = 1;

/// The envelope schema generation this build writes.
pub const SCHEMA_VERSION: u32 = 1;

/// The oldest schema generation this build still accepts.
pub const SUPPORTED_SCHEMA_MIN: u32 = 1;

/// The newest schema generation this build accepts.
pub const SUPPORTED_SCHEMA_MAX: u32 = 1;

/// Whether a protocol string is compatible with this build.
///
/// The major is the compatibility unit: `bsh.fed/1` and `bsh.fed/1.x` are
/// accepted, `bsh.fed/2` is a breaking change and is refused with a reason.
pub fn accepts_protocol(protocol: &str) -> bool {
    protocol
        .strip_prefix("bsh.fed/")
        .and_then(|rest| rest.split('.').next())
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major == PROTOCOL_MAJOR)
}

/// Whether a schema generation is inside the supported window.
pub fn accepts_schema(schema: u32) -> bool {
    (SUPPORTED_SCHEMA_MIN..=SUPPORTED_SCHEMA_MAX).contains(&schema)
}

/// How long a message stays valid when it carries no expiry of its own.
pub const DEFAULT_LIFETIME_MS: u64 = 60_000;

/// The tolerated clock skew for a message issued in the future.
pub const CLOCK_SKEW_MS: u64 = 60_000;

/// One signed federation message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Protocol name and version.
    pub protocol: String,
    /// Envelope schema generation.
    pub schema_version: u32,
    /// Message identity.
    pub message_id: String,
    /// Message kind: invoke, result, event, cancel, discover.
    pub kind: String,
    /// The authenticated sender.
    pub sender: NodeId,
    /// The intended recipient.
    pub recipient: NodeId,
    /// When it was issued, in milliseconds since the Unix epoch.
    pub issued_at_ms: u64,
    /// When it stops being valid.
    pub expires_at_ms: Option<u64>,
    /// Correlation shared by a request and its result.
    pub correlation: Option<String>,
    /// Deduplication key, when the operation supports one.
    pub idempotency_key: Option<String>,
    /// MIME type of the payload.
    pub content_type: String,
    /// The payload itself.
    pub payload: Value,
    /// Base64 Ed25519 signature over the canonical form.
    pub signature: String,
}

/// Sort object keys recursively, so the signed bytes do not depend on the
/// order a JSON parser happened to preserve.
fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            let mut sorted = serde_json::Map::new();
            for (key, value) in entries {
                sorted.insert(key.clone(), canonical_value(value));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_value).collect()),
        other => other.clone(),
    }
}
/// Why an envelope was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeError {
    /// The bytes are not an envelope.
    #[error("the envelope is not valid JSON: {0}")]
    Json(String),
    /// The protocol name or version is not ours.
    #[error("unsupported protocol {found}; this build speaks {expected}")]
    UnknownProtocol {
        /// What the envelope declared.
        found: String,
        /// What this build speaks.
        expected: String,
    },
    /// The schema generation is outside the supported window.
    #[error("unsupported schema {found}; this build accepts {min}..={max}")]
    UnsupportedSchema {
        /// What the envelope declared.
        found: u32,
        /// The oldest accepted generation.
        min: u32,
        /// The newest accepted generation.
        max: u32,
    },
    /// The signature does not verify.
    #[error("the envelope signature does not verify")]
    Signature,
    /// The envelope is past its expiry.
    #[error("the envelope expired")]
    Expired,
    /// The envelope claims to be issued in the future.
    #[error("the envelope is not yet valid")]
    NotYetValid,
    /// The payload is not JSON.
    #[error("the payload is not JSON: {0}")]
    Payload(String),
}

impl Envelope {
    /// Seal one message for a recipient.
    pub fn seal(
        message: &FederationMessage,
        sender: &NodeIdentity,
        recipient: &NodeId,
    ) -> Result<Self, EnvelopeError> {
        Self::seal_at(message, sender, recipient, unix_ms())
    }

    /// Seal with an explicit issue time.
    ///
    /// Interoperability vectors call this: a signature is only reproducible
    /// when the issue time is fixed.
    pub fn seal_at(
        message: &FederationMessage,
        sender: &NodeIdentity,
        recipient: &NodeId,
        issued_at_ms: u64,
    ) -> Result<Self, EnvelopeError> {
        let payload: Value = serde_json::from_slice(&message.payload)
            .map_err(|error| EnvelopeError::Payload(error.to_string()))?;
        let mut envelope = Self {
            protocol: PROTOCOL.to_string(),
            schema_version: SCHEMA_VERSION,
            message_id: message.id.as_str().to_string(),
            kind: message.kind.clone(),
            sender: sender.node_id(),
            recipient: recipient.clone(),
            issued_at_ms,
            expires_at_ms: message
                .expires_at_ms
                .or(Some(issued_at_ms.saturating_add(DEFAULT_LIFETIME_MS))),
            correlation: message.correlation.clone(),
            idempotency_key: message.idempotency_key.clone(),
            content_type: "application/json".to_string(),
            payload,
            signature: String::new(),
        };
        envelope.signature = sender.sign(&envelope.canonical());
        Ok(envelope)
    }

    /// Verify the signature against the sender's Node id.
    pub fn verify(&self) -> bool {
        identity::verify(&self.sender, &self.canonical(), &self.signature)
    }

    /// Check protocol, schema, signature, and validity, then return the payload.
    pub fn open(&self, now_ms: u64) -> Result<&Value, EnvelopeError> {
        if !accepts_protocol(&self.protocol) {
            return Err(EnvelopeError::UnknownProtocol {
                found: self.protocol.clone(),
                expected: PROTOCOL.to_string(),
            });
        }
        if !accepts_schema(self.schema_version) {
            return Err(EnvelopeError::UnsupportedSchema {
                found: self.schema_version,
                min: SUPPORTED_SCHEMA_MIN,
                max: SUPPORTED_SCHEMA_MAX,
            });
        }
        if !self.verify() {
            return Err(EnvelopeError::Signature);
        }
        if self.expires_at_ms.is_some_and(|expires| now_ms >= expires) {
            return Err(EnvelopeError::Expired);
        }
        if self.issued_at_ms > now_ms.saturating_add(CLOCK_SKEW_MS) {
            return Err(EnvelopeError::NotYetValid);
        }
        Ok(&self.payload)
    }

    /// Serialize the envelope to the bytes a transport carries.
    pub fn encode(&self) -> Result<Vec<u8>, EnvelopeError> {
        serde_json::to_vec(self).map_err(|error| EnvelopeError::Json(error.to_string()))
    }

    /// Decode an envelope from transport bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, EnvelopeError> {
        serde_json::from_slice(bytes).map_err(|error| EnvelopeError::Json(error.to_string()))
    }

    /// The bytes the signature covers: the envelope with the signature cleared.
    fn canonical(&self) -> Vec<u8> {
        self.canonical_bytes()
    }

    /// The exact bytes a signature covers.
    ///
    /// The signature field is empty while signing, and `serde_json` writes the
    /// fields in declaration order (see `docs/federation-signing.md`), so two
    /// implementations that agree on the struct agree on these bytes.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.signature = String::new();
        // Object keys are sorted recursively: two parsers that disagree on map
        // order must still sign the same bytes.
        unsigned.payload = canonical_value(&unsigned.payload);
        serde_json::to_vec(&unsigned).expect("an envelope serializes")
    }

    /// The signature as raw bytes, for a caller that stores it inline.
    pub fn signature_bytes(&self) -> Option<Vec<u8>> {
        STANDARD.decode(&self.signature).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::NodeIdentity;

    fn sealed() -> Envelope {
        let sender = NodeIdentity::from_seed([71u8; 32]);
        let recipient = NodeIdentity::from_seed([72u8; 32]).node_id();
        let message = FederationMessage::new(
            "invoke",
            br#"{"capability":"builtin/echo","input":{}}"#.to_vec(),
        );
        Envelope::seal_at(&message, &sender, &recipient, 1_000).expect("a sealed envelope")
    }

    #[test]
    fn the_protocol_window_is_major_based() {
        assert!(accepts_protocol("bsh.fed/1"));
        assert!(accepts_protocol("bsh.fed/1.4"));
        assert!(!accepts_protocol("bsh.fed/2"));
        assert!(!accepts_protocol("bsh.fing/1"));
        assert!(accepts_schema(1));
        assert!(!accepts_schema(0));
        assert!(!accepts_schema(2));
    }

    #[test]
    fn an_unsupported_protocol_is_named() {
        let mut envelope = sealed();
        envelope.protocol = "bsh.fed/2".to_string();
        match envelope.open(1_000) {
            Err(EnvelopeError::UnknownProtocol { found, expected }) => {
                assert_eq!(found, "bsh.fed/2");
                assert_eq!(expected, PROTOCOL);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn an_unsupported_schema_names_the_window() {
        let mut envelope = sealed();
        envelope.schema_version = 7;
        match envelope.open(1_000) {
            Err(EnvelopeError::UnsupportedSchema { found, min, max }) => {
                assert_eq!(found, 7);
                assert_eq!((min, max), (SUPPORTED_SCHEMA_MIN, SUPPORTED_SCHEMA_MAX));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn additive_and_unknown_fields_are_tolerated() {
        let envelope = sealed();
        let mut value = serde_json::to_value(&envelope).expect("json");
        value["future_field"] = serde_json::json!({ "x": 1 });
        let bytes = serde_json::to_vec(&value).expect("bytes");
        let decoded = Envelope::decode(&bytes).expect("decode");
        assert!(
            decoded.open(1_000).is_ok(),
            "an unknown field does not break a signed envelope"
        );
    }
}
