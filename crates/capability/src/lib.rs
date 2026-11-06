#![warn(missing_docs)]
//! The capability domain: identity, descriptors, the provider contract, and the
//! two-level composition resolver.
//!
//! There are two kinds of composition, and they share one model:
//!
//! - **Local composition** assembles capabilities inside one Workspace. It has
//!   one Security Kernel, one lifecycle, and one authoritative state.
//! - **Federated composition** resolves a plan over autonomous Nodes. A remote
//!   capability is a binding to another Node's kernel, never a local provider,
//!   and no Workspace spans Nodes.
//!
//! What the two share is everything below the binding: capability ids,
//! descriptors, the invocation contract, and the error taxonomy. What they do
//! not share is authority, state, lifecycle, isolation, or transport.
//!
//! The crate is deliberately dependency-free of the agent stack: no react, no
//! agent, no ferris, no UI. That is what lets the Composition Engine live above
//! it without depending on a specific Agent implementation.

pub mod catalogue;
pub mod composition;
pub mod context;
pub mod descriptor;
pub mod error;
pub mod id;
pub mod provider;
pub mod snapshot;
pub mod transport;
pub mod workspace;

pub use catalogue::Catalogue;
pub use composition::{
    resolve, Binding, CapabilityRequirement, CompositionError, CompositionId, CompositionSpec,
    GrantRef, LocalScope, PeerCatalogue, PeerRef, RemotePolicy, ResolvedCapability,
    ResolvedComposition,
};
pub use context::{
    CancelToken, DelegationHop, InvocationContext, InvocationId, PolicyDecisionRef, Principal,
    TaskId, TraceContext,
};
pub use descriptor::{
    CapabilityDescriptor, FeatureSet, InvocationLimits, OutputContract, Permission, Risk,
    SideEffect, Visibility,
};
pub use error::{CapabilityError, DeliveryState};
pub use id::{
    CapabilityId, CapabilityKind, InvalidId, NodeId, ProviderId, SurfaceName, VersionSource,
    WorkspaceId,
};
pub use provider::{
    CapabilityCall, CapabilityContent, CapabilityOutput, CapabilityProvider, ProviderHealth,
    ProviderKind, Usage,
};
pub use snapshot::{CapabilitySnapshot, CompositionSnapshot, SnapshotOrigin};
pub use transport::{unix_ms, FederationMessage, FederationTransport, MessageId, PeerSession};
pub use workspace::{remote_provider_id, WorkspaceError, WorkspaceRuntime};
