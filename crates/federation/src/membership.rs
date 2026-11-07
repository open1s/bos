//! Federation membership metadata.
//!
//! A federation is a policy and trust domain, not a superuser. Membership is
//! advertised and recorded beside the per-peer trust decision; it never grants
//! a peer authority by itself. The policy revision names the version of the
//! rules a Node is applying, so a reader can tell a stale membership view from
//! a current one.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable identity of one federation trust domain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FederationId(String);

impl FederationId {
    /// Build a federation id.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FederationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
