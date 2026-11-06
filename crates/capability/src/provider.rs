//! The provider contract.
//!
//! Every implementation surface — a native tool, a Ferris runtime, an MCP
//! server, a remote Node, a resource — is lowered into these two operations:
//! describe what is offered, and invoke one of them.

use crate::context::{InvocationContext, InvocationId};
use crate::descriptor::CapabilityDescriptor;
use crate::error::CapabilityError;
use crate::id::{CapabilityId, NodeId, ProviderId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// How a provider is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderKind {
    /// Native Rust, in this process.
    Builtin,
    /// A Ferris extension runtime.
    Ferris,
    /// An MCP server.
    Mcp,
    /// A WASM component.
    Wasm,
    /// An external process.
    Process,
    /// Another BSH Node. A remote capability is a proxy, not a local provider.
    Remote {
        /// The Node that actually executes it.
        node: NodeId,
    },
    /// A resource subsystem endpoint.
    Resource,
}

/// One part of a result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CapabilityContent {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// Structured JSON.
    Json {
        /// The value.
        value: serde_json::Value,
    },
    /// A file, base64 encoded.
    File {
        /// Display name.
        name: String,
        /// MIME type.
        media_type: String,
        /// Base64 bytes.
        data: String,
    },
    /// An image, base64 encoded.
    Image {
        /// MIME type.
        media_type: String,
        /// Base64 bytes.
        data: String,
        /// Alternative text.
        alt: String,
    },
}

/// Metered usage, when a provider reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Input units consumed, in whatever unit the provider meters.
    pub input_units: Option<u64>,
    /// Output units produced.
    pub output_units: Option<u64>,
}

/// What one invocation produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityOutput {
    /// Ordered content parts.
    pub content: Vec<CapabilityContent>,
    /// Machine-readable value, when the capability has one.
    pub structured: Option<serde_json::Value>,
    /// Metered usage.
    pub usage: Option<Usage>,
}

impl CapabilityOutput {
    /// A single text result.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![CapabilityContent::Text { text: text.into() }],
            structured: None,
            usage: None,
        }
    }

    /// A structured JSON result.
    pub fn json(value: serde_json::Value) -> Self {
        Self {
            content: vec![CapabilityContent::Json {
                value: value.clone(),
            }],
            structured: Some(value),
            usage: None,
        }
    }
}

/// One invocation request.
#[derive(Debug, Clone)]
pub struct CapabilityCall<'a> {
    /// The capability being invoked.
    pub capability: &'a CapabilityId,
    /// The operation, for capabilities that expose more than one.
    pub operation: Option<&'a str>,
    /// The arguments, already validated against the descriptor schema.
    pub input: serde_json::Value,
    /// Who is acting, under which deadline, and on whose authority.
    pub context: &'a InvocationContext,
}

/// What a provider says about itself right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderHealth {
    /// Reachable and serving.
    Available,
    /// Reachable, serving degraded.
    Degraded {
        /// Why it is degraded.
        reason: String,
    },
    /// Not serving.
    Unavailable {
        /// Why it is unavailable.
        reason: String,
    },
}

/// The contract every implementation surface is lowered into.
#[async_trait]
pub trait CapabilityProvider: Send + Sync {
    /// Stable provider identity.
    fn id(&self) -> &ProviderId;

    /// How this provider is reached.
    fn kind(&self) -> ProviderKind;

    /// Everything this provider currently offers.
    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError>;

    /// Invoke one capability.
    async fn invoke(&self, call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError>;

    /// Stop one in-flight invocation, if it is still running.
    async fn cancel(&self, _invocation: &InvocationId) -> Result<(), CapabilityError> {
        Ok(())
    }

    /// Availability, without promising that a later call will succeed.
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}
