//! bsh own capabilities, declared through the framework plugin registry.
//!
//! `agent::plugins` is the host-side extension seam: a `Plugin` contributes
//! tools, panels and commands, and `PluginRegistry` is where they land with an
//! owner recorded against every item. bsh capabilities - the shell, the file
//! tools and the plan tool - are exactly such bundles, so they are declared
//! here and the registry, not a second list, is what the agent is built from.

use std::sync::Arc;

use agent::plugins::{Plugin, PluginRegistry};
use agent::tools::Tool;

/// A plugin the settings pane lists, with the tools it contributed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginInfo {
    /// The plugin name.
    pub(crate) name: String,
    /// Tool names it contributed, in registry order. Empty while disabled.
    pub(crate) tools: Vec<String>,
    /// Whether the settings let it install at all.
    pub(crate) enabled: bool,
}

/// One capability bundle, declared as a plugin.
pub(crate) struct Bundle {
    name: &'static str,
    tools: Vec<Arc<dyn Tool>>,
}

impl Bundle {
    /// Bundle the tools that survived the enable/disable filters under `name`.
    pub(crate) fn new(name: &'static str, tools: Vec<Arc<dyn Tool>>) -> Self {
        Self { name, tools }
    }
}

impl Plugin for Bundle {
    fn name(&self) -> &str {
        self.name
    }

    fn register(&self, registry: &mut PluginRegistry) {
        for tool in &self.tools {
            // A duplicate means bsh built the same tool twice, which the
            // registry reports; nothing here depends on the late one winning.
            let _ = registry.register_tool(self.name, tool.clone());
        }
    }
}

/// Install the enabled bundles and report every bundle, grouped by owner.
///
/// A bundle named in the disabled list is left out of the registry, so the
/// tools it would have contributed never reach the agent; it is still
/// reported, with no tools and enabled false, because the pane has to list it
/// to switch it back on.
pub(crate) fn install(
    bundles: Vec<Bundle>,
    disabled: &[String],
) -> (PluginRegistry, Vec<PluginInfo>) {
    let mut registry = PluginRegistry::new();
    for bundle in &bundles {
        if disabled.iter().any(|name| name == bundle.name) {
            continue;
        }
        let _ = registry.install(bundle);
    }
    let plugins = bundles
        .iter()
        .map(|bundle| PluginInfo {
            name: bundle.name.to_string(),
            enabled: !disabled.iter().any(|name| name == bundle.name),
            tools: registry
                .tool_names()
                .into_iter()
                .filter(|tool| {
                    registry
                        .owner_of(tool)
                        .is_some_and(|(_, owner)| owner == bundle.name)
                })
                .collect(),
        })
        .collect();
    (registry, plugins)
}
