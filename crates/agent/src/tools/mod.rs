//! Built-in tools, the tool registry, and schema helpers.

/// Shell command execution tool.
pub mod bash;
/// Adapters that turn plain functions into tools.
pub mod function;
/// Registry that stores and executes tools by name.
pub mod registry;
/// Renders JSON schemas as human-readable text.
pub mod translator;
/// Validates tool arguments against a JSON schema.
pub mod validator;

pub use bash::BashTool;
pub use function::FunctionTool;
pub use registry::ToolRegistry;
pub use translator::describe_schema;
pub use validator::validate_args;

pub use react::tool::Tool;
pub use react::tool::ToolError;
