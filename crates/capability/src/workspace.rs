//! An instantiated workspace: a resolved plan plus the providers it dispatches to.
//!
//! The runtime is the same shape at both levels. A local binding dispatches to
//! the provider that implements the capability here; a remote binding
//! dispatches to the provider registered for that peer, which proxies the call
//! across the federation. The workspace never decides which one it is holding:
//! the binding does, and it was decided by the resolver under policy.
//!
//! A workspace is one instance with one id. Using a peer does not create a
//! second workspace, and it does not merge the peer's authority into this one.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::composition::{Binding, ResolvedComposition};
use crate::descriptor::CapabilityDescriptor;
use crate::error::CapabilityError;
use crate::id::{CapabilityId, NodeId, ProviderId, WorkspaceId};
use crate::provider::{CapabilityCall, CapabilityOutput, CapabilityProvider};

/// The provider id a remote binding dispatches through.
///
/// A remote capability is a proxy: the peer's provider adapter registers under
/// this id and performs the authenticated federation call. The capability's own
/// id is not rewritten, because it still names the peer's capability.
pub fn remote_provider_id(node: &NodeId) -> ProviderId {
    ProviderId::new(format!("remote:{node}"))
}

/// Why a workspace could not be instantiated.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WorkspaceError {
    /// A binding named a provider that is not registered.
    #[error("no provider is registered for {provider}")]
    MissingProvider {
        /// The provider the plan needs.
        provider: ProviderId,
    },
    /// The plan binds one capability more than once.
    #[error("the plan binds {capability} more than once")]
    Duplicate {
        /// The capability bound twice.
        capability: CapabilityId,
    },
}

struct Dispatch {
    descriptor: CapabilityDescriptor,
    provider: Arc<dyn CapabilityProvider>,
    binding: Binding,
}

/// One instantiated workspace.
pub struct WorkspaceRuntime {
    workspace: WorkspaceId,
    capabilities: BTreeMap<CapabilityId, Dispatch>,
}

impl std::fmt::Debug for WorkspaceRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceRuntime")
            .field("workspace", &self.workspace)
            .field(
                "capabilities",
                &self.capabilities.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl WorkspaceRuntime {
    /// Bind a resolved plan to the providers that will execute it.
    ///
    /// A local binding needs its provider id; a remote binding needs the
    /// provider registered for that peer. Nothing here reaches a peer itself,
    /// so a missing remote provider fails at instantiation rather than on the
    /// first call.
    pub fn instantiate(
        plan: ResolvedComposition,
        providers: &[Arc<dyn CapabilityProvider>],
    ) -> Result<Self, WorkspaceError> {
        let mut capabilities = BTreeMap::new();
        for resolved in plan.capabilities {
            let id = resolved.descriptor.id.clone();
            let provider_id = match &resolved.binding {
                Binding::Local { provider } => provider.clone(),
                Binding::Remote { node, .. } => remote_provider_id(node),
            };
            let provider = providers
                .iter()
                .find(|provider| provider.id() == &provider_id)
                .cloned()
                .ok_or(WorkspaceError::MissingProvider {
                    provider: provider_id,
                })?;
            let dispatch = Dispatch {
                descriptor: resolved.descriptor,
                provider,
                binding: resolved.binding,
            };
            if capabilities.insert(id.clone(), dispatch).is_some() {
                return Err(WorkspaceError::Duplicate { capability: id });
            }
        }
        Ok(Self {
            workspace: plan.workspace,
            capabilities,
        })
    }

    /// The workspace this runtime instantiates.
    pub fn workspace(&self) -> &WorkspaceId {
        &self.workspace
    }

    /// The descriptors this workspace exposes, in id order.
    pub fn describe(&self) -> Vec<&CapabilityDescriptor> {
        self.capabilities
            .values()
            .map(|dispatch| &dispatch.descriptor)
            .collect()
    }

    /// Where one capability executes.
    pub fn binding(&self, id: &CapabilityId) -> Option<&Binding> {
        self.capabilities.get(id).map(|dispatch| &dispatch.binding)
    }

    /// Whether one capability is part of this workspace.
    pub fn contains(&self, id: &CapabilityId) -> bool {
        self.capabilities.contains_key(id)
    }

    /// Invoke one capability.
    ///
    /// An id outside the workspace is a NotFound, not a denial: the workspace
    /// exposes a subset, and a caller cannot ask it for something it was not
    /// built with.
    pub async fn invoke(
        &self,
        id: &CapabilityId,
        input: serde_json::Value,
        context: &crate::context::InvocationContext,
    ) -> Result<CapabilityOutput, CapabilityError> {
        self.invoke_operation(id, None, input, context).await
    }

    /// Invoke one operation of a multi-operation capability.
    pub async fn invoke_operation(
        &self,
        id: &CapabilityId,
        operation: Option<&str>,
        input: serde_json::Value,
        context: &crate::context::InvocationContext,
    ) -> Result<CapabilityOutput, CapabilityError> {
        // A Workspace is an authority boundary, not just a capability
        // lookup table. Do not let a context minted for another Workspace
        // borrow this runtime's provider bindings.
        if context.workspace != self.workspace {
            return Err(CapabilityError::Denied {
                reason: format!(
                    "invocation context belongs to workspace {}, not {}",
                    context.workspace, self.workspace
                ),
                policy: None,
            });
        }

        let Some(dispatch) = self.capabilities.get(id) else {
            return Err(CapabilityError::NotFound {
                capability: id.clone(),
            });
        };
        dispatch
            .provider
            .invoke(CapabilityCall {
                capability: id,
                operation,
                input,
                context,
            })
            .await
    }
}
