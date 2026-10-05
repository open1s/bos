//! Tool trait, registry, and JSON-schema descriptors.

/// OpenAI-style JSON-schema descriptors for tools.
pub mod descriptor;
/// The canonical tool error type.
pub mod error;
/// Tool traits and the in-memory registry.
pub mod registry;

pub use descriptor::{ToolDefinition, ToolFunction, ToolParameterProperty, ToolParameters};
pub use error::ToolError;
pub use registry::{FnTool, Tool, ToolRegistry};
