use std::sync::Arc;

use async_trait::async_trait;

use crate::error::ToolError;

pub mod registry;
pub mod translator;
pub mod validator;
pub mod bus_client;

pub use registry::ToolRegistry;
pub use translator::describe_schema;
pub use validator::validate_args;
pub use bus_client::BusToolClient;

#[derive(Debug, Clone)]
pub struct ToolDescription {
    pub short: String,
    pub parameters: String,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> ToolDescription;
    fn json_schema(&self) -> serde_json::Value;
    
    /// Get cached JSON schema as Arc for zero-copy access
    /// Default implementation wraps json_schema() in Arc
    fn cached_schema(&self) -> Arc<serde_json::Value> {
        Arc::new(self.json_schema())
    }
    
    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError>;
}

#[async_trait]
impl Tool for Box<dyn Tool> {
    fn name(&self) -> &str {
        (**self).name()
    }

    fn description(&self) -> ToolDescription {
        (**self).description()
    }

    fn json_schema(&self) -> serde_json::Value {
        (**self).json_schema()
    }

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        (**self).execute(args).await
    }
}

#[async_trait]
impl Tool for Arc<dyn Tool> {
    fn name(&self) -> &str {
        (**self).name()
    }

    fn description(&self) -> ToolDescription {
        (**self).description()
    }

    fn json_schema(&self) -> serde_json::Value {
        (**self).json_schema()
    }

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        (**self).execute(args).await
    }
}
