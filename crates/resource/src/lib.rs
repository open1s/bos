//! Resource Abstract Definition layer.
//!
//! A common, secure, network-aware access layer for AI agents. Every
//! interaction is a single typed message: `invoke(ResourceAction) ->
//! Result<ResourceOutput, ResourceError>`. Resources are addressed by URI and
//! **operated identically whether they live locally or on another machine** —
//! network location is just part of the URI (`scheme://host/...`), never part of
//! the calling code.
//!
//! # Seamless local ↔ network access
//!
//! [`ResourceClient`] is the high-level entry point. It resolves each call in
//! this order:
//!
//! 1. **Local** — a co-located [`ResourceManager`] that has the URI registered.
//! 2. **Remote by host** — `register_remote(host, transport)` maps a URI's
//!    authority to the right endpoint (e.g. QUIC), so `invoke("file://node1/x")`
//!    reaches `node1` transparently.
//! 3. **Default remote** — a fallback transport for any unmatched URI.
//!
//! The remote transport is pluggable ([`Transport`]); the bundled [`transport`]
//! module ships an in-process dispatcher and a QUIC transport with
//! certificate-based agent identity and `(agent, uri, action)` policy
//! enforcement on the server side.
//!
//! A [`Resource`] handle pins a single URI to a client for repeated operations
//! without re-passing the address — again identical for local and remote.

pub mod action;
pub mod client;
pub mod debug;
pub mod discovery;
pub mod error;
pub mod explorer;
pub mod handler;
pub mod manager;
pub mod meta;
pub mod net;
pub mod platform;
pub mod policy;
pub mod resource;
pub mod tool;
pub mod transport;

pub use action::{
    decode_action, decode_output, encode_action, encode_output, ResourceAction, ResourceEvent,
    ResourceOutput,
};
pub use client::{Resource, ResourceClient};
pub use discovery::{NodeAnnounce, DISCOVERY_TOPIC};
pub use resource::combine::CombineResource;
pub use resource::vnode::VirtualNodeResource;
pub use debug::init_logging;
pub use error::{ResourceError, Result};
pub use explorer::{handler_for, Explorer, Row};
pub use handler::ResourceHandler;
pub use manager::ResourceManager;
pub use meta::{ResourceInfo, ResourceMeta, ResourceStateLabel, ResourceType};
pub use net::Net;
pub use platform::{Os, Terminate};
pub use policy::{PolicyDoc, Rule, SharedPolicy, Effect};
pub use resource::{
    file::FileResource, folder::FolderResource, mem::MemResource,
    proc::{manager::ProcManager, supervisor::{ChildSpec, RestartPolicy, SupPolicy, Supervisor}, ProcResource},
    sock::SockResource,
};
pub use tool::ResourceTool;
pub use transport::{Chunk, ChunkStream, ChunkWriter, Dispatcher, Transport};

/// Convenience prelude.
pub mod prelude {
    pub use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
    pub use crate::client::{Resource, ResourceClient};
    pub use crate::error::{ResourceError, Result};
    pub use crate::explorer::{handler_for, Explorer, Row};
    pub use crate::handler::ResourceHandler;
    pub use crate::manager::ResourceManager;
    pub use crate::meta::{ResourceInfo, ResourceMeta, ResourceType};
    pub use crate::net::Net;
    pub use crate::policy::{PolicyDoc, SharedPolicy};
    pub use crate::resource::{
        combine::CombineResource, file::FileResource, folder::FolderResource,
        mem::MemResource,
        proc::{manager::ProcManager, supervisor::{ChildSpec, RestartPolicy, SupPolicy, Supervisor}, ProcResource},
        sock::SockResource, vnode::VirtualNodeResource,
    };
    pub use crate::tool::ResourceTool;
    pub use crate::transport::{Chunk, ChunkStream, ChunkWriter, Dispatcher, Transport};
}