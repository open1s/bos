//! The transport envelope for surface frames, patches and actions.
//!
//! The framework does not own a transport. This module defines the bounded,
//! versioned message a bus or a federation transport carries, so both ends
//! agree on size, version and refusal before a frame reaches an engine.
//!
//! A remote message is untrusted: the receiving host validates the frame and
//! the bindings again. The envelope only proves what kind of message it is.

use serde::{Deserialize, Serialize};

use super::engine::SurfaceEvent;
use super::frame::SurfaceFrame;
use super::{Generation, InstanceId, Revision, SurfaceError};

/// The wire schema this build writes and reads.
pub const SURFACE_WIRE_VERSION: &str = "1.0";

/// The largest message this build accepts by default.
pub const MAX_WIRE_BYTES: usize = 2 * 1024 * 1024;

/// One surface action request as it crosses a transport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceAction {
    /// The instance the action targets.
    pub instance: InstanceId,
    /// The generation the control was rendered from.
    pub generation: Generation,
    /// The revision the control was rendered from.
    pub revision: Revision,
    /// The binding key the control named.
    pub binding: String,
    /// The event the control reported.
    pub event: SurfaceEvent,
}

/// What a surface transport carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "message", rename_all = "kebab-case")]
pub enum SurfaceMessage {
    /// A full frame or a base-relative patch.
    Frame {
        /// The frame, boxed because a document dwarfs every other message.
        frame: Box<SurfaceFrame>,
    },
    /// An action from a control.
    Action {
        /// The action.
        action: SurfaceAction,
    },
    /// A request to dispose an instance.
    Close {
        /// The instance.
        instance: InstanceId,
        /// The generation to close.
        generation: Generation,
    },
}

/// A versioned envelope around one message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceEnvelope {
    /// The wire schema version.
    pub wire_version: String,
    /// The message.
    #[serde(flatten)]
    pub message: SurfaceMessage,
}

/// Byte ceilings applied to one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireLimits {
    /// Largest accepted message.
    pub max_bytes: usize,
}

impl Default for WireLimits {
    fn default() -> Self {
        Self {
            max_bytes: MAX_WIRE_BYTES,
        }
    }
}

/// Encode one message, refusing anything over the ceiling.
pub fn encode(message: &SurfaceMessage) -> Result<Vec<u8>, SurfaceError> {
    encode_with(message, &WireLimits::default())
}

/// Encode with explicit limits.
pub fn encode_with(message: &SurfaceMessage, limits: &WireLimits) -> Result<Vec<u8>, SurfaceError> {
    let envelope = SurfaceEnvelope {
        wire_version: SURFACE_WIRE_VERSION.to_string(),
        message: message.clone(),
    };
    let bytes = serde_json::to_vec(&envelope)
        .map_err(|error| SurfaceError::Invalid(format!("message cannot be serialized: {error}")))?;
    if bytes.len() > limits.max_bytes {
        return Err(SurfaceError::Budget(format!(
            "message is {} bytes; the ceiling is {}",
            bytes.len(),
            limits.max_bytes
        )));
    }
    Ok(bytes)
}

/// Decode one message, enforcing size and wire version.
pub fn decode(bytes: &[u8]) -> Result<SurfaceMessage, SurfaceError> {
    decode_with(bytes, &WireLimits::default())
}

/// Decode with explicit limits.
pub fn decode_with(bytes: &[u8], limits: &WireLimits) -> Result<SurfaceMessage, SurfaceError> {
    if bytes.len() > limits.max_bytes {
        return Err(SurfaceError::Budget(format!(
            "message is {} bytes; the ceiling is {}",
            bytes.len(),
            limits.max_bytes
        )));
    }
    let envelope: SurfaceEnvelope = serde_json::from_slice(bytes)
        .map_err(|error| SurfaceError::Invalid(format!("message is not valid: {error}")))?;
    if envelope.wire_version != SURFACE_WIRE_VERSION {
        return Err(SurfaceError::Invalid(format!(
            "wire version {:?} is not {:?}",
            envelope.wire_version, SURFACE_WIRE_VERSION
        )));
    }
    Ok(envelope.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::{Revision, SurfaceLimits};

    fn frame() -> SurfaceFrame {
        let document = crate::surface::SurfaceDocument {
            schema_version: crate::surface::document::LEGACY_SCHEMA_VERSION.to_string(),
            bindings: std::collections::BTreeMap::new(),
            hidden_actions: Vec::new(),
            root: crate::surface::SurfaceNode::Text {
                id: None,
                text: "wire".to_string(),
                tone: crate::ui::Tone::Neutral,
            },
        };
        SurfaceFrame::full(
            InstanceId::new(),
            Generation::initial(),
            Revision::initial(),
            document,
        )
    }

    #[test]
    fn a_frame_round_trips() {
        let message = SurfaceMessage::Frame {
            frame: Box::new(frame()),
        };
        let bytes = encode(&message).expect("encode");
        assert_eq!(decode(&bytes).expect("decode"), message);
    }

    #[test]
    fn an_action_round_trips() {
        let message = SurfaceMessage::Action {
            action: SurfaceAction {
                instance: InstanceId::new(),
                generation: Generation::initial(),
                revision: Revision::initial(),
                binding: "refresh".to_string(),
                event: SurfaceEvent {
                    value: Some("x".to_string()),
                    checked: Some(true),
                    option: None,
                },
            },
        };
        let bytes = encode(&message).expect("encode");
        assert_eq!(decode(&bytes).expect("decode"), message);
    }

    #[test]
    fn an_oversized_message_is_refused() {
        let limits = WireLimits { max_bytes: 8 };
        assert!(matches!(
            encode_with(
                &SurfaceMessage::Close {
                    instance: InstanceId::new(),
                    generation: Generation::initial(),
                },
                &limits
            ),
            Err(SurfaceError::Budget(_))
        ));
        assert!(matches!(
            decode_with(b"{}", &WireLimits { max_bytes: 1 }),
            Err(SurfaceError::Budget(_))
        ));
    }

    #[test]
    fn a_foreign_wire_version_is_refused() {
        let bytes = br#"{"wire_version":"9.9","message":"close","instance":"00000000-0000-4000-8000-000000000000","generation":1}"#;
        let error = decode(bytes).expect_err("a foreign version is refused");
        assert!(error.to_string().contains("wire version"));
    }

    #[test]
    fn the_default_limits_match_the_document_ceiling() {
        assert_eq!(WireLimits::default().max_bytes, MAX_WIRE_BYTES);
        assert!(SurfaceLimits::default().max_bytes < MAX_WIRE_BYTES);
    }
}
