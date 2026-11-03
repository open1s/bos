#![warn(missing_docs)]
//! Standard runtimes provided by the BOS extension framework.
//!
//! Phase 0 defines the runtime catalog and the feature boundary. Individual
//! runtime implementations are migrated from BSH in Phase 2.

pub mod adapter;
pub mod bridge;
pub mod builtin;
pub mod contract;
#[cfg(all(feature = "extism", feature = "wasm", feature = "cordis"))]
pub mod factory;
pub use contract::*;
#[cfg(feature = "cordis")]
pub mod cordis;
#[cfg(feature = "declarative")]
pub mod declarative;
#[cfg(feature = "extism")]
pub mod extism;
pub mod hook;
pub mod hooks;
#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(feature = "process")]
pub mod process;
#[cfg(feature = "wasm")]
#[allow(missing_docs)] // bindgen generates the WIT world types.
pub mod wasm;
#[cfg(feature = "workflow")]
pub mod workflow;

use std::collections::BTreeMap;

use bos_ext_core::{ExtError, RuntimeKindSpec};

/// Runtime kinds compiled into this crate with default features.
pub const STANDARD_KINDS: &[&str] = &["builtin", "declarative", "process"];

/// Optional runtime kinds behind feature flags.
pub const OPTIONAL_KINDS: &[&str] = &["wasm", "extism", "mcp", "cordis"];

/// Host-owned registry of runtime kind constructors.
#[derive(Default)]
pub struct RuntimeCatalog {
    specs: BTreeMap<String, RuntimeKindSpec>,
}

impl RuntimeCatalog {
    /// Create an empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one runtime kind.
    pub fn register(&mut self, spec: RuntimeKindSpec) {
        self.specs.insert(spec.kind.clone(), spec);
    }

    /// Look up one runtime kind.
    pub fn get(&self, kind: &str) -> Option<&RuntimeKindSpec> {
        self.specs.get(kind)
    }

    /// Whether the kind is registered.
    pub fn contains(&self, kind: &str) -> bool {
        self.specs.contains_key(kind)
    }

    /// Registered kind ids.
    pub fn kinds(&self) -> Vec<&str> {
        self.specs.keys().map(String::as_str).collect()
    }

    /// Build one runtime, or a named refusal.
    pub fn create(
        &self,
        kind: &str,
        root: &std::path::Path,
    ) -> Result<std::sync::Arc<dyn bos_ext_core::Runtime>, ExtError> {
        let spec = self.get(kind).ok_or_else(|| ExtError {
            code: bos_ext_core::ErrorCode::Runtime,
            message: format!("runtime kind {kind} is not registered"),
            details: serde_json::Value::Null,
        })?;
        (spec.create)(&root.to_path_buf())
    }
}
