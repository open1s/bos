//! Re-exported from bos-ext-core; ownership moved in Phase 1.
pub(crate) use bos_ext_core::types::*;

/// BSH adapter: map a framework error to the agent tool error the model sees.
pub(crate) trait ToToolError {
    /// Map to the agent tool error.
    fn to_tool_error(&self) -> agent::tools::ToolError;
}

impl ToToolError for ExtensionError {
    fn to_tool_error(&self) -> agent::tools::ToolError {
        match self {
            Self::Denied { .. } | Self::MissingCapability { .. } => {
                agent::tools::ToolError::Failed(format!("permission denied: {self}"))
            }
            Self::Timeout { .. } => agent::tools::ToolError::Timeout(self.to_string()),
            Self::NotFound { id } => agent::tools::ToolError::NotFound(id.clone()),
            Self::Limit { .. } => agent::tools::ToolError::Failed(self.to_string()),
            Self::InvalidId { .. }
            | Self::InvalidContribution { .. }
            | Self::InvalidVersion { .. }
            | Self::Manifest { .. } => agent::tools::ToolError::InvalidInput(self.to_string()),
            _ => agent::tools::ToolError::Failed(self.to_string()),
        }
    }
}
