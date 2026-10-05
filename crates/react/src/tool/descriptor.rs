use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// One property in a tool parameter schema.
pub struct ToolParameterProperty {
    /// JSON type of the property.
    #[serde(rename = "type")]
    pub param_type: String,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional allowed values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// The parameter object of a tool definition.
pub struct ToolParameters {
    /// JSON type, normally `object`.
    #[serde(rename = "type")]
    pub param_type: String,
    /// Property schemas by name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<std::collections::HashMap<String, ToolParameterProperty>>,
    /// Names of required properties.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// The function block of an OpenAI-style tool definition.
pub struct ToolFunction {
    /// Function name.
    pub name: String,
    /// Function description.
    pub description: String,
    /// Optional parameter schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<ToolParameters>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// An OpenAI-style tool definition.
pub struct ToolDefinition {
    /// Tool type, normally `function`.
    #[serde(rename = "type")]
    pub tool_type: String,
    /// The function block.
    pub function: ToolFunction,
}

impl ToolDefinition {
    /// Create a definition for a bare function.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: ToolFunction {
                name: name.into(),
                description: description.into(),
                parameters: None,
            },
        }
    }

    /// Attach a parameter schema.
    pub fn with_parameters(mut self, parameters: ToolParameters) -> Self {
        self.function.parameters = Some(parameters);
        self
    }
}
