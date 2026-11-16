//! Workspace identity and root descriptors.
//!
//! These types establish identity boundaries only. They do not provide a
//! filesystem sandbox; safe file traversal and race-resistant file opening are
//! implemented by the filesystem layer in a later story.

mod identity;

pub use identity::{WorkspaceDescriptor, WorkspaceError, WorkspaceId, WorkspaceRegistry};
