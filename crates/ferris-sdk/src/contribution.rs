//! Typed contribution declarations for a 2.0 manifest.

/// Host-registered contribution kinds used by the standard runtimes.
pub mod kinds {
    /// Agent-callable tool.
    pub const TOOL: &str = "tool";
    /// Host command.
    pub const COMMAND: &str = "command";
    /// Host-managed panel.
    pub const PANEL: &str = "panel";
    /// Prompt fragment.
    pub const PROMPT: &str = "prompt";
    /// Scoped context provider.
    pub const CONTEXT: &str = "context";
    /// SKILL.md skill directory.
    pub const SKILL: &str = "skill";
    /// Reusable workflow.
    pub const WORKFLOW: &str = "workflow";
    /// Visual theme.
    pub const THEME: &str = "theme";
    /// Agent/module preset.
    pub const PRESET: &str = "preset";
    /// Model provider descriptor.
    pub const MODEL_PROVIDER: &str = "model-provider";
    /// Agent role descriptor.
    pub const AGENT_PROVIDER: &str = "agent-provider";
    /// Language service descriptor.
    pub const LANGUAGE: &str = "language";
    /// External-system integration descriptor.
    pub const INTEGRATION: &str = "integration";
    /// Event subscription.
    pub const EVENT_SUBSCRIBE: &str = "event.subscribe";
    /// Event publication.
    pub const EVENT_PUBLISH: &str = "event.publish";
    /// Observe hook.
    pub const HOOK: &str = "hook";
}

/// One contribution entry in a 2.0 manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct ContributionSpec {
    /// Host-registered kind.
    pub kind: String,
    /// Contribution id inside the extension.
    pub id: String,
    /// Opaque payload validated by the host.
    pub payload: Option<toml::Value>,
}

impl ContributionSpec {
    /// A contribution of one kind.
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
            payload: None,
        }
    }

    /// Set the opaque payload.
    #[must_use]
    pub fn payload(mut self, payload: toml::Value) -> Self {
        self.payload = Some(payload);
        self
    }

    /// An agent tool.
    pub fn tool(id: impl Into<String>) -> Self {
        Self::new(kinds::TOOL, id)
    }

    /// A host command.
    pub fn command(id: impl Into<String>) -> Self {
        Self::new(kinds::COMMAND, id)
    }

    /// A host panel.
    pub fn panel(id: impl Into<String>) -> Self {
        Self::new(kinds::PANEL, id)
    }

    /// A prompt fragment.
    pub fn prompt(id: impl Into<String>) -> Self {
        Self::new(kinds::PROMPT, id)
    }

    /// A scoped context provider.
    pub fn context(id: impl Into<String>) -> Self {
        Self::new(kinds::CONTEXT, id)
    }

    /// A skill directory.
    pub fn skill(id: impl Into<String>) -> Self {
        Self::new(kinds::SKILL, id)
    }

    /// A workflow.
    pub fn workflow(id: impl Into<String>) -> Self {
        Self::new(kinds::WORKFLOW, id)
    }

    /// A theme.
    pub fn theme(id: impl Into<String>) -> Self {
        Self::new(kinds::THEME, id)
    }

    /// An agent/module preset.
    pub fn preset(id: impl Into<String>) -> Self {
        Self::new(kinds::PRESET, id)
    }

    /// A model provider descriptor.
    pub fn model_provider(id: impl Into<String>) -> Self {
        Self::new(kinds::MODEL_PROVIDER, id)
    }

    /// An agent role descriptor.
    pub fn agent_provider(id: impl Into<String>) -> Self {
        Self::new(kinds::AGENT_PROVIDER, id)
    }

    /// A language service descriptor.
    pub fn language(id: impl Into<String>) -> Self {
        Self::new(kinds::LANGUAGE, id)
    }

    /// An external-system integration.
    pub fn integration(id: impl Into<String>) -> Self {
        Self::new(kinds::INTEGRATION, id)
    }

    /// An event subscription.
    pub fn subscribe(topic: impl Into<String>) -> Self {
        Self::new(kinds::EVENT_SUBSCRIBE, topic)
    }

    /// An event publication.
    pub fn publish(topic: impl Into<String>) -> Self {
        Self::new(kinds::EVENT_PUBLISH, topic)
    }

    /// An observe hook.
    pub fn hook(point: impl Into<String>) -> Self {
        Self::new(kinds::HOOK, point)
    }

    /// Convert to a runtime contribution with a JSON payload.
    pub fn into_contribution(self, payload: serde_json::Value) -> ferris_core::Contribution {
        ferris_core::Contribution {
            kind: self.kind,
            id: self.id,
            payload,
        }
    }

    pub(crate) fn into_table(self) -> toml::map::Map<String, toml::Value> {
        let mut table = toml::map::Map::new();
        table.insert("kind".to_string(), toml::Value::String(self.kind));
        table.insert("id".to_string(), toml::Value::String(self.id));
        if let Some(payload) = self.payload {
            table.insert("payload".to_string(), payload);
        }
        table
    }
}
