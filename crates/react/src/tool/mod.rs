//! Tool trait, registry, and JSON-schema descriptors.

/// Admission control for the tool definitions one request advertises.
pub mod admission;
/// OpenAI-style JSON-schema descriptors for tools.
pub mod descriptor;
/// The canonical tool error type.
pub mod error;
/// Static metadata a tool declares about its capability contract.
pub mod metadata;
/// Tool traits and the in-memory registry.
pub mod registry;
/// The shared result envelope every tool result is rendered through.
pub mod result;

pub use admission::{
    Admission, AdmissionPolicy, DEFAULT_DISCOVERY_RESERVE, WithheldReason, WithheldTool, admit,
};
pub use descriptor::{ToolDefinition, ToolFunction, ToolParameterProperty, ToolParameters};
pub use error::{ToolError, ToolErrorCode};
pub use metadata::ToolMetadata;
pub use registry::{FnTool, Tool, ToolRegistry};
pub use result::{ToolContinuation, ToolErrorInfo, ToolResult, ToolStatus};
