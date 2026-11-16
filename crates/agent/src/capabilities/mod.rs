//! Bridging the engine's tool registry and the capability model.
//!
//! Two directions live here:
//!
//! - [catalogue_from_registry] lowers registered tools into descriptors. It
//!   reads what the tool actually carries, so a catalogue cannot describe a
//!   tool the registry does not have.
//! - [NativeToolProvider] executes a capability call through the same
//!   registry, with a deadline, a cancellation id, and classified failures.
//!
//! Neither decides policy. A call arrives with a context that already carries
//! the decision that admitted it.

mod catalogue;
mod native;
mod shim;

pub use catalogue::{builtin_contract_version, builtin_provider, catalogue_from_registry};
pub use native::NativeToolProvider;
pub use shim::{to_tool_error, CapabilityTool, ProxySource, ToolFacts};
