//! Plugin seam, declarative presets, and registry collision detection.
//!
//! A [`Plugin`] is a small unit of extension that contributes up to three kinds
//! of item to a host: a tool, a UI panel, and a command. [`PluginRegistry`]
//! stores all three in one namespace and rejects a name that is already taken by
//! *any* kind, reporting both the previous owner and the incoming owner so the
//! conflict is actionable.
//!
//! [`presets`] holds the declarative preset format and its field-level
//! validation errors.

/// Declarative preset loading and validation.
pub mod presets;

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;

use crate::tools::{Tool, ToolError};

/// The kind of item a plugin can contribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// A tool the model can call.
    Tool,
    /// A UI panel.
    Panel,
    /// A host command.
    Command,
}

impl std::fmt::Display for ItemKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ItemKind::Tool => write!(f, "tool"),
            ItemKind::Panel => write!(f, "panel"),
            ItemKind::Command => write!(f, "command"),
        }
    }
}

/// A UI panel contributed by a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiPanel {
    /// Unique panel id.
    pub id: String,
    /// Human-readable panel title.
    pub title: String,
}

impl UiPanel {
    /// Create a panel with `id` and `title`.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
        }
    }
}

/// A host command contributed by a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    /// Unique command name.
    pub name: String,
    /// Human-readable command description.
    pub description: String,
}

impl PluginCommand {
    /// Create a command with `name` and `description`.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

/// Errors raised while installing plugins.
#[derive(Debug, Clone, Error)]
pub enum PluginError {
    /// A name is already registered, possibly by a different item kind or plugin.
    #[error(
        "name conflict for {kind} '{name}': already registered by plugin \
         '{existing_owner}' as {existing_kind}, cannot register from plugin \
         '{incoming_owner}'"
    )]
    NameConflict {
        /// The duplicated name.
        name: String,
        /// The kind the incoming registration wanted.
        kind: ItemKind,
        /// The name of the plugin that registered the existing item.
        existing_owner: String,
        /// The kind of the existing item.
        existing_kind: ItemKind,
        /// The name of the plugin attempting the registration.
        incoming_owner: String,
    },
}

/// A unit of extension that contributes tools, panels, and commands.
pub trait Plugin: Send + Sync + 'static {
    /// Unique plugin name; also the owner recorded against its items.
    fn name(&self) -> &str;

    /// Contribute items to `registry`.
    ///
    /// Registrations are made through [`PluginRegistry::register_tool`],
    /// [`PluginRegistry::register_panel`], and
    /// [`PluginRegistry::register_command`]. Each returns a [`PluginError`] on a
    /// name conflict, which [`PluginRegistry::install`] surfaces to its caller.
    fn register(&self, registry: &mut PluginRegistry);
}

/// Stores the tools, panels, and commands contributed by plugins.
///
/// All three kinds share one namespace: a tool, panel, and command cannot share
/// a name. [`PluginRegistry::register_tool`] and friends return
/// [`PluginError::NameConflict`] on a duplicate.
#[derive(Default)]
pub struct PluginRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    panels: HashMap<String, UiPanel>,
    commands: HashMap<String, PluginCommand>,
    owners: HashMap<String, (ItemKind, String)>,
    errors: Vec<PluginError>,
}

impl PluginRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Install `plugin`, returning the first name conflict it raised.
    ///
    /// An install is all-or-nothing: a plugin that registers some items and
    /// then hits a conflict has those earlier registrations rolled back, so a
    /// failed install never leaves half a plugin in the namespace.
    pub fn install(&mut self, plugin: &dyn Plugin) -> Result<(), PluginError> {
        self.errors.clear();
        let snapshot = (
            self.tools.clone(),
            self.panels.clone(),
            self.commands.clone(),
            self.owners.clone(),
        );
        plugin.register(self);
        if let Some(error) = self.errors.first().cloned() {
            self.tools = snapshot.0;
            self.panels = snapshot.1;
            self.commands = snapshot.2;
            self.owners = snapshot.3;
            self.errors.clear();
            Err(error)
        } else {
            self.errors.clear();
            Ok(())
        }
    }

    fn claim(&mut self, name: &str, kind: ItemKind, owner: &str) -> Result<(), PluginError> {
        if let Some((existing_kind, existing_owner)) = self.owners.get(name) {
            let error = PluginError::NameConflict {
                name: name.to_string(),
                kind,
                existing_owner: existing_owner.clone(),
                existing_kind: *existing_kind,
                incoming_owner: owner.to_string(),
            };
            self.errors.push(error.clone());
            return Err(error);
        }
        self.owners
            .insert(name.to_string(), (kind, owner.to_string()));
        Ok(())
    }

    /// Register `tool` on behalf of `owner`.
    pub fn register_tool(&mut self, owner: &str, tool: Arc<dyn Tool>) -> Result<(), PluginError> {
        let name = tool.name().to_string();
        self.claim(&name, ItemKind::Tool, owner)?;
        self.tools.insert(name, tool);
        Ok(())
    }

    /// Register `panel` on behalf of `owner`.
    pub fn register_panel(&mut self, owner: &str, panel: UiPanel) -> Result<(), PluginError> {
        self.claim(&panel.id, ItemKind::Panel, owner)?;
        self.panels.insert(panel.id.clone(), panel);
        Ok(())
    }

    /// Register `command` on behalf of `owner`.
    pub fn register_command(
        &mut self,
        owner: &str,
        command: PluginCommand,
    ) -> Result<(), PluginError> {
        self.claim(&command.name, ItemKind::Command, owner)?;
        self.commands.insert(command.name.clone(), command);
        Ok(())
    }

    /// Look up a registered tool by name.
    pub fn tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Names of the registered tools, sorted.
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        names.sort();
        names
    }

    /// Registered panels, sorted by id.
    pub fn panels(&self) -> Vec<&UiPanel> {
        let mut panels: Vec<&UiPanel> = self.panels.values().collect();
        panels.sort_by(|a, b| a.id.cmp(&b.id));
        panels
    }

    /// Registered commands, sorted by name.
    pub fn commands(&self) -> Vec<&PluginCommand> {
        let mut commands: Vec<&PluginCommand> = self.commands.values().collect();
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        commands
    }

    /// The kind and owner plugin recorded for `name`, if registered.
    pub fn owner_of(&self, name: &str) -> Option<(ItemKind, &str)> {
        self.owners
            .get(name)
            .map(|(kind, owner)| (*kind, owner.as_str()))
    }

    /// Total number of registered items across all kinds.
    pub fn len(&self) -> usize {
        self.owners.len()
    }

    /// Whether no item is registered.
    pub fn is_empty(&self) -> bool {
        self.owners.is_empty()
    }
}

impl std::fmt::Debug for PluginRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRegistry")
            .field("tools", &self.tool_names())
            .field(
                "panels",
                &self.panels().iter().map(|p| &p.id).collect::<Vec<_>>(),
            )
            .field(
                "commands",
                &self.commands().iter().map(|c| &c.name).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// A trivial echo tool used by [`SamplePlugin`] and tests.
#[derive(Debug, Clone)]
pub struct EchoTool {
    name: String,
}

impl EchoTool {
    /// Create an echo tool with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Tool for EchoTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        format!("Echo the `value` argument back ({})", self.name)
    }

    fn run(&self, input: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        Ok(serde_json::json!({
            "value": input.get("value").cloned().unwrap_or(serde_json::Value::Null),
            "tool": self.name,
        }))
    }

    fn json_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
        })
    }
}

/// A minimal plugin contributing one tool, one panel, and one command.
pub struct SamplePlugin;

impl SamplePlugin {
    /// Create the sample plugin.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SamplePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for SamplePlugin {
    fn name(&self) -> &str {
        "sample"
    }

    fn register(&self, registry: &mut PluginRegistry) {
        let _ = registry.register_tool(self.name(), Arc::new(EchoTool::new("sample_echo")));
        let _ = registry.register_panel(self.name(), UiPanel::new("sample_panel", "Sample Panel"));
        let _ = registry.register_command(
            self.name(),
            PluginCommand::new("sample_cmd", "Run the sample command"),
        );
    }
}

/// Convenience constructor for [`SamplePlugin`].
pub fn sample_plugin() -> SamplePlugin {
    SamplePlugin::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NamedPlugin {
        name: String,
    }

    /// Every `NamedPlugin` registers the same tool name, so two of them collide.
    const SHARED_TOOL: &str = "shared_tool";

    impl Plugin for NamedPlugin {
        fn name(&self) -> &str {
            &self.name
        }

        fn register(&self, registry: &mut PluginRegistry) {
            let _ = registry.register_tool(self.name(), Arc::new(EchoTool::new(SHARED_TOOL)));
        }
    }

    struct CrossKindPlugin {
        name: String,
    }

    impl Plugin for CrossKindPlugin {
        fn name(&self) -> &str {
            &self.name
        }

        fn register(&self, registry: &mut PluginRegistry) {
            let _ = registry.register_panel(self.name(), UiPanel::new(SHARED_TOOL, "Cross"));
        }
    }

    #[test]
    fn sample_plugin_registers_one_of_each() {
        let mut registry = PluginRegistry::new();
        registry
            .install(&sample_plugin())
            .expect("install succeeds");

        assert_eq!(registry.len(), 3);
        assert_eq!(registry.tool_names(), vec!["sample_echo".to_string()]);
        assert_eq!(registry.panels().len(), 1);
        assert_eq!(registry.panels()[0].id, "sample_panel");
        assert_eq!(registry.commands().len(), 1);
        assert_eq!(registry.commands()[0].name, "sample_cmd");

        // The registered tool is callable.
        let tool = registry.tool("sample_echo").expect("tool present");
        let out = tool.run(&serde_json::json!({"value": "hi"})).unwrap();
        assert_eq!(out["value"], "hi");
    }

    #[test]
    fn sample_plugin_owner_is_recorded() {
        let mut registry = PluginRegistry::new();
        registry.install(&sample_plugin()).unwrap();

        assert_eq!(
            registry.owner_of("sample_echo"),
            Some((ItemKind::Tool, "sample"))
        );
        assert_eq!(
            registry.owner_of("sample_panel"),
            Some((ItemKind::Panel, "sample"))
        );
        assert_eq!(
            registry.owner_of("sample_cmd"),
            Some((ItemKind::Command, "sample"))
        );
    }

    #[test]
    fn collision_is_rejected_and_names_both_owners() {
        let mut registry = PluginRegistry::new();
        let alpha = NamedPlugin {
            name: "alpha".into(),
        };
        let beta = NamedPlugin {
            name: "beta".into(),
        };

        registry.install(&alpha).expect("alpha installs first");

        let error = registry.install(&beta).expect_err("beta collides");
        let message = error.to_string();
        assert!(message.contains("alpha"), "{message}");
        assert!(message.contains("beta"), "{message}");
        assert!(message.contains("shared_tool"), "{message}");
        assert!(message.contains("tool"), "{message}");

        // The failed install left the registry untouched.
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.owner_of("shared_tool"),
            Some((ItemKind::Tool, "alpha"))
        );
    }

    struct PartialPlugin;

    impl Plugin for PartialPlugin {
        fn name(&self) -> &str {
            "partial"
        }

        fn register(&self, registry: &mut PluginRegistry) {
            // The tool lands before the panel collides on the shared name.
            let _ = registry.register_tool(self.name(), Arc::new(EchoTool::new("partial_tool")));
            let _ = registry.register_panel(self.name(), UiPanel::new(SHARED_TOOL, "Cross"));
        }
    }

    #[test]
    fn a_failed_install_rolls_back_earlier_items() {
        let mut registry = PluginRegistry::new();
        registry
            .install(&NamedPlugin {
                name: "alpha".into(),
            })
            .expect("alpha installs first");

        let error = registry
            .install(&PartialPlugin)
            .expect_err("panel collides");
        assert!(error.to_string().contains(SHARED_TOOL), "{error}");

        // The tool registered before the collision is not left behind.
        assert_eq!(registry.len(), 1);
        assert!(registry.tool("partial_tool").is_none());
        assert_eq!(registry.owner_of("partial_tool"), None);
    }

    #[test]
    fn collision_across_kinds_is_rejected() {
        let mut registry = PluginRegistry::new();
        let tool_plugin = NamedPlugin {
            name: "alpha".into(),
        };
        let panel_plugin = CrossKindPlugin {
            name: "beta".into(),
        };

        registry.install(&tool_plugin).unwrap();

        let error = registry.install(&panel_plugin).expect_err("panel collides");
        let message = error.to_string();
        assert!(message.contains("shared_tool"), "{message}");
        assert!(message.contains("panel"), "{message}");
        assert!(message.contains("alpha"), "{message}");
        assert!(message.contains("beta"), "{message}");
    }

    #[test]
    fn direct_registration_reports_conflict() {
        let mut registry = PluginRegistry::new();
        registry
            .register_tool("alpha", Arc::new(EchoTool::new("dup")))
            .unwrap();

        let error = registry
            .register_command("beta", PluginCommand::new("dup", "dup command"))
            .expect_err("command collides with tool");
        match error {
            PluginError::NameConflict {
                name,
                kind,
                existing_kind,
                existing_owner,
                incoming_owner,
            } => {
                assert_eq!(name, "dup");
                assert_eq!(kind, ItemKind::Command);
                assert_eq!(existing_kind, ItemKind::Tool);
                assert_eq!(existing_owner, "alpha");
                assert_eq!(incoming_owner, "beta");
            }
        }
    }

    #[test]
    fn distinct_names_all_register() {
        let mut registry = PluginRegistry::new();
        registry
            .register_tool("a", Arc::new(EchoTool::new("t1")))
            .unwrap();
        registry
            .register_panel("a", UiPanel::new("p1", "Panel"))
            .unwrap();
        registry
            .register_command("a", PluginCommand::new("c1", "Command"))
            .unwrap();

        assert_eq!(registry.len(), 3);
        assert!(!registry.is_empty());
        assert!(registry.tool_names().len() == 1);
    }
}
