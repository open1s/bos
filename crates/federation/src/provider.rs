//! A peer's capabilities as one local provider.
//!
//! The workspace dispatches by binding, not by catalogue: a capability bound to
//! a peer is executed by this provider, registered under the peer's remote
//! provider id. The descriptor keeps the peer's own id, because it still names
//! the peer's capability; the binding is what makes it remote (ADR-007).
//!
//! There is no fallback. If the peer cannot be reached, the call fails as a
//! transport failure; it never resolves to a local capability that happens to
//! share the id.

use std::sync::Arc;

use async_trait::async_trait;
use capability::{
    remote_provider_id, CapabilityCall, CapabilityDescriptor, CapabilityError, CapabilityOutput,
    CapabilityProvider, ProviderHealth, ProviderId, ProviderKind,
};

use crate::service::FederationNode;

/// One peer, exposed as the provider a workspace dispatches remote bindings to.
pub struct RemoteCapabilityProvider {
    node: capability::NodeId,
    provider: ProviderId,
    federation: Arc<FederationNode>,
    descriptors: Vec<CapabilityDescriptor>,
}

impl RemoteCapabilityProvider {
    /// Build the provider for one peer from the descriptors that peer offered.
    pub fn new(
        node: capability::NodeId,
        federation: Arc<FederationNode>,
        descriptors: Vec<CapabilityDescriptor>,
    ) -> Self {
        Self {
            provider: remote_provider_id(&node),
            node,
            federation,
            descriptors,
        }
    }

    /// The peer this provider speaks to.
    pub fn node(&self) -> &capability::NodeId {
        &self.node
    }
}

#[async_trait]
impl CapabilityProvider for RemoteCapabilityProvider {
    fn id(&self) -> &ProviderId {
        &self.provider
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Remote {
            node: self.node.clone(),
        }
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(self.descriptors.clone())
    }

    async fn invoke(&self, call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        // The generic adapter sends the scope convention a scoped grant can
        // enforce: the capability's leaf name as the operation, and its path or
        // uri argument (when it has one) as the resource. A legacy grant with no
        // selectors admits either value.
        let scope = crate::InvocationScope {
            operation: Some(call.capability.name().to_string()),
            resource: call
                .input
                .get("path")
                .or_else(|| call.input.get("uri"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        };
        self.federation
            .invoke_scoped(
                &self.node,
                call.capability,
                call.input.clone(),
                Some(&scope),
                call.context,
            )
            .await
    }

    async fn health(&self) -> ProviderHealth {
        // A probe would be a network round trip per call; RFC section 12.4 says
        // availability is decided at invocation, so this does not claim more
        // than it can prove.
        ProviderHealth::Available
    }
}
