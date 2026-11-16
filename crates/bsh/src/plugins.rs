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
    /// Whether the bundle installed successfully AND the settings let it.
    /// A failed installation is never reported as enabled (issue #328).
    pub(crate) enabled: bool,
    /// The installation error, when the registry refused the bundle. The pane
    /// shows it instead of claiming the plugin is healthy.
    pub(crate) error: Option<String>,
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
    let mut failures: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for bundle in &bundles {
        if disabled.iter().any(|name| name == bundle.name) {
            continue;
        }
        // The registry rolls a conflicting bundle back whole; the host must
        // surface that failure, not discard it and report the bundle enabled
        // with none of its tools installed (issue #328).
        if let Err(error) = registry.install(bundle) {
            log::warn!("plugin bundle '{}' failed to install: {error}", bundle.name);
            failures.insert(bundle.name.to_string(), error.to_string());
        }
    }
    let plugins = bundles
        .iter()
        .map(|bundle| {
            let error = failures.get(bundle.name).cloned();
            let enabled = !disabled.iter().any(|name| name == bundle.name) && error.is_none();
            PluginInfo {
                name: bundle.name.to_string(),
                enabled,
                error,
                tools: registry
                    .tool_names()
                    .into_iter()
                    .filter(|tool| {
                        registry
                            .owner_of(tool)
                            .is_some_and(|(_, owner)| owner == bundle.name)
                    })
                    .collect(),
            }
        })
        .collect();
    (registry, plugins)
}

#[cfg(test)]
mod tests {
    use super::*;
    use react::tool::{Tool, ToolError};

    struct NamedTool(&'static str);
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> String {
            "test".to_string()
        }
        fn run(&self, _args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
            Ok(serde_json::json!({}))
        }
    }

    /// A conflicting bundle is reported failed, not enabled, and its tools
    /// are absent from the registry (issue #328).
    #[test]
    fn a_conflicting_bundle_is_visible_and_not_enabled() {
        let first = Bundle::new("alpha", vec![Arc::new(NamedTool("shared"))]);
        let second = Bundle::new("beta", vec![Arc::new(NamedTool("shared"))]);
        let (registry, plugins) = install(vec![first, second], &[]);
        let alpha = plugins.iter().find(|p| p.name == "alpha").unwrap();
        let beta = plugins.iter().find(|p| p.name == "beta").unwrap();
        assert!(alpha.enabled);
        assert!(alpha.error.is_none());
        assert!(!beta.enabled, "a failed bundle must not report enabled");
        assert!(beta.error.is_some(), "the failure must be visible");
        assert!(beta.tools.is_empty());
        // The registry kept the first claimant only.
        assert_eq!(registry.owner_of("shared").map(|(_, o)| o), Some("alpha"));
    }

    /// A disabled bundle is listed but not installed, with no error.
    #[test]
    fn a_disabled_bundle_is_listed_without_error() {
        let bundle = Bundle::new("alpha", vec![Arc::new(NamedTool("tool"))]);
        let (_registry, plugins) = install(vec![bundle], &["alpha".to_string()]);
        let alpha = &plugins[0];
        assert!(!alpha.enabled);
        assert!(alpha.error.is_none());
        assert!(alpha.tools.is_empty());
    }
}
