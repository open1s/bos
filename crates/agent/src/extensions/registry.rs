//! The one registry both standards register into.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::agent::Agent;
use crate::plugins::{Plugin, PluginCommand, PluginRegistry, UiPanel};
use crate::tools::Tool;

use super::{
    codex, dsh, Capability, Contribution, ContributionKind, Extension, ExtensionError,
    ExtensionStandard, HookContribution, PromptContribution, ProviderContribution, Refusal,
    SkillContribution, SkillDirContribution, Trust,
};

/// A parsed package: the extension plus every part the shim refused.
#[derive(Debug, Clone)]
pub struct LoadedPackage {
    /// The extension whose declarative parts loaded.
    pub extension: Extension,
    /// Explicit refusals for parts that need a runtime this build does not have.
    pub refusals: Vec<Refusal>,
    /// The live bridge session, when a TRUSTED package's host entry ran.
    ///
    /// It is the only handle that keeps the plugin's process alive, so a host
    /// that wants the tools it registered to stay CALLABLE must hold it: drop it
    /// and the process exits and every bridged tool starts failing. A package
    /// with no imperative entry, or an untrusted one, carries `None` and never
    /// held a process at all.
    pub session: Option<std::sync::Arc<std::sync::Mutex<super::bridge::BridgeSession>>>,
}

impl LoadedPackage {
    /// Create a loaded package.
    pub fn new(extension: Extension, refusals: Vec<Refusal>) -> Self {
        Self {
            extension,
            refusals,
            session: None,
        }
    }

    /// Whether every declared part loaded.
    pub fn is_fully_supported(&self) -> bool {
        self.refusals.is_empty()
    }

    /// Consume the package and return the extension, ignoring refusals.
    pub fn into_extension(self) -> Extension {
        self.extension
    }

    /// Consume the package, failing on the first refusal.
    ///
    /// This is the strict path a caller uses when a partially loaded package is
    /// not acceptable: the error is the named [`Refusal`], not a generic
    /// failure.
    pub fn into_strict(self) -> Result<Extension, ExtensionError> {
        if let Some(refusal) = self.refusals.into_iter().next() {
            Err(ExtensionError::Refused(refusal))
        } else {
            Ok(self.extension)
        }
    }
}

/// The contributions [`ExtensionRegistry::apply_to_agent`] installed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppliedExtensions {
    /// Tool names registered on the agent.
    pub tools: Vec<String>,
    /// Hook plugin names registered on the agent.
    pub hooks: Vec<String>,
    /// Individual skills registered on the agent.
    pub skills: Vec<String>,
    /// Skill directories registered on the agent.
    pub skill_dirs: Vec<String>,
}

/// The host commands and panels [`ExtensionRegistry::install_host_items`]
/// installed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostInstall {
    /// Command names registered in the host plugin registry.
    pub commands: Vec<String>,
    /// Panel ids registered in the host plugin registry.
    pub panels: Vec<String>,
}

/// One registry for every loaded extension, whatever standard it came from.
#[derive(Debug, Clone, Default)]
pub struct ExtensionRegistry {
    extensions: BTreeMap<String, Extension>,
    owners: HashMap<(ContributionKind, String), String>,
    refusals: Vec<Refusal>,
}

impl ExtensionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse the package at `root`, detecting its standard.
    pub fn load(root: &Path) -> Result<LoadedPackage, ExtensionError> {
        match ExtensionStandard::detect(root)? {
            ExtensionStandard::DeepSeekHarness => dsh::load(root),
            ExtensionStandard::Codex => codex::load(root),
        }
    }

    /// Parse the package at `root`, reading its declared parts and deciding
    /// whether its imperative entry may be EXECUTED.
    ///
    /// This is the one entry point both trust states share. An
    /// [`Trust::Untrusted`] package is parsed exactly like [`Self::load`]:
    /// its declarative contributions load, its entry is refused by name, and
    /// no process starts. A [`Trust::Trusted`] package's entry runs in the
    /// bridge and its registrations land in the same contribution list.
    pub fn load_with_trust(root: &Path, trust: Trust) -> Result<LoadedPackage, ExtensionError> {
        match ExtensionStandard::detect(root)? {
            ExtensionStandard::DeepSeekHarness => dsh::load_with_trust(root, trust),
            // The Codex standard has no imperative entry in its manifest, so
            // trust changes nothing about how it is read; the decision still
            // gates the MCP servers it declares, which the host applies.
            ExtensionStandard::Codex => codex::load(root),
        }
    }

    /// Parse, register, and return the extension at `root`, honouring `trust`.
    pub fn install_package_with_trust(
        &mut self,
        root: &Path,
        trust: Trust,
    ) -> Result<&Extension, ExtensionError> {
        let package = Self::load_with_trust(root, trust)?;
        let id = package.extension.id.clone();
        self.install(package)?;
        self.get_required(&id)
    }
    /// Parse the package at `root` as `standard`.
    ///
    /// A package that is actually the other standard is refused with
    /// [`ExtensionError::WrongStandard`] naming both sides; it is never
    /// re-read under the wrong field names.
    pub fn load_as(
        root: &Path,
        standard: ExtensionStandard,
    ) -> Result<LoadedPackage, ExtensionError> {
        let found = ExtensionStandard::detect(root)?;
        if found != standard {
            return Err(ExtensionError::WrongStandard {
                path: root.to_path_buf(),
                expected: standard,
                found,
            });
        }
        match standard {
            ExtensionStandard::DeepSeekHarness => dsh::load(root),
            ExtensionStandard::Codex => codex::load(root),
        }
    }

    /// Parse the package at `root` and fail on the first refusal.
    pub fn load_strict(root: &Path) -> Result<Extension, ExtensionError> {
        Self::load(root)?.into_strict()
    }

    /// Parse, register, and return the extension at `root`.
    pub fn install_package(&mut self, root: &Path) -> Result<&Extension, ExtensionError> {
        let package = Self::load(root)?;
        let id = package.extension.id.clone();
        self.install(package)?;
        self.get_required(&id)
    }

    /// Parse, register, and return the extension at `root`, failing on any
    /// refusal.
    pub fn install_package_strict(&mut self, root: &Path) -> Result<&Extension, ExtensionError> {
        let extension = Self::load(root)?.into_strict()?;
        let id = extension.id.clone();
        self.add(extension)?;
        self.get_required(&id)
    }

    fn get_required(&self, id: &str) -> Result<&Extension, ExtensionError> {
        self.extensions
            .get(id)
            .ok_or_else(|| ExtensionError::UnknownExtension { id: id.to_string() })
    }

    /// Register a parsed package and record its refusals.
    pub fn install(&mut self, package: LoadedPackage) -> Result<(), ExtensionError> {
        self.add(package.extension)?;
        self.refusals.extend(package.refusals);
        Ok(())
    }

    /// Register one extension and all of its contributions atomically.
    ///
    /// Name collisions are checked before anything is inserted, so a failed
    /// registration leaves the registry untouched.
    pub fn add(&mut self, extension: Extension) -> Result<(), ExtensionError> {
        if self.extensions.contains_key(&extension.id) {
            return Err(ExtensionError::DuplicateExtension {
                id: extension.id.clone(),
            });
        }
        let mut local: HashMap<(ContributionKind, String), String> = HashMap::new();
        for contribution in &extension.contributions {
            let key = (contribution.kind(), contribution.name());
            if let Some(existing) = self.owners.get(&key).or_else(|| local.get(&key)) {
                return Err(ExtensionError::DuplicateContribution {
                    kind: key.0,
                    name: key.1,
                    existing: existing.clone(),
                    incoming: extension.id.clone(),
                });
            }
            local.insert(key, extension.id.clone());
        }
        self.owners.extend(local);
        self.extensions.insert(extension.id.clone(), extension);
        Ok(())
    }

    /// Add one contribution to an already registered extension.
    pub fn add_contribution(
        &mut self,
        extension_id: &str,
        contribution: Contribution,
    ) -> Result<(), ExtensionError> {
        if !self.extensions.contains_key(extension_id) {
            return Err(ExtensionError::UnknownExtension {
                id: extension_id.to_string(),
            });
        }
        let key = (contribution.kind(), contribution.name());
        if let Some(existing) = self.owners.get(&key) {
            return Err(ExtensionError::DuplicateContribution {
                kind: key.0,
                name: key.1,
                existing: existing.clone(),
                incoming: extension_id.to_string(),
            });
        }
        if let Some(extension) = self.extensions.get_mut(extension_id) {
            extension.contributions.push(contribution);
        }
        self.owners.insert(key, extension_id.to_string());
        Ok(())
    }

    /// Load every package directly under `root`, installing each.
    ///
    /// Directories that carry neither standard marker are skipped; a package
    /// that does carry a marker but fails to parse aborts the walk with its own
    /// error.
    pub fn discover(&mut self, root: &Path) -> Result<Vec<String>, ExtensionError> {
        let mut directories = Vec::new();
        for entry in std::fs::read_dir(root).map_err(|source| ExtensionError::Io {
            path: root.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| ExtensionError::Io {
                path: root.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
            }
        }
        directories.sort();
        let mut installed = Vec::new();
        for directory in directories {
            if !Self::is_package(&directory)? {
                continue;
            }
            let package = Self::load(&directory)?;
            let id = package.extension.id.clone();
            self.install(package)?;
            installed.push(id);
        }
        Ok(installed)
    }

    fn is_package(root: &Path) -> Result<bool, ExtensionError> {
        if root.join(".codex-plugin").join("plugin.json").is_file() {
            return Ok(true);
        }
        super::package_json_has_dsh(root)
    }

    /// Every registered extension, ordered by id.
    pub fn extensions(&self) -> Vec<&Extension> {
        self.extensions.values().collect()
    }

    /// One extension by id.
    pub fn get(&self, id: &str) -> Option<&Extension> {
        self.extensions.get(id)
    }

    /// Extensions from one standard.
    pub fn by_standard(&self, standard: ExtensionStandard) -> Vec<&Extension> {
        self.extensions
            .values()
            .filter(|extension| extension.standard == standard)
            .collect()
    }

    /// Extensions that declared `capability`.
    pub fn with_capability(&self, capability: Capability) -> Vec<&Extension> {
        self.extensions
            .values()
            .filter(|extension| extension.has_capability(capability))
            .collect()
    }

    /// Whether any extension declared `capability`.
    pub fn has_capability(&self, capability: Capability) -> bool {
        self.extensions
            .values()
            .any(|extension| extension.has_capability(capability))
    }

    /// Every contribution, paired with its owner id.
    pub fn contributions(&self) -> Vec<(&str, &Contribution)> {
        let mut contributions = Vec::new();
        for extension in self.extensions.values() {
            for contribution in &extension.contributions {
                contributions.push((extension.id.as_str(), contribution));
            }
        }
        contributions
    }

    /// The contributions of one extension.
    pub fn contributions_of(&self, id: &str) -> &[Contribution] {
        self.extensions
            .get(id)
            .map(|extension| extension.contributions.as_slice())
            .unwrap_or(&[])
    }

    /// Every tool, ordered by name.
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let mut tools: Vec<Arc<dyn Tool>> = self
            .contributions()
            .into_iter()
            .filter_map(|(_, contribution)| match contribution {
                Contribution::Tool(tool) => Some(Arc::clone(tool)),
                _ => None,
            })
            .collect();
        tools.sort_by(|left, right| left.name().cmp(right.name()));
        tools
    }

    /// Tool names, ordered by name.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools()
            .into_iter()
            .map(|tool| tool.name().to_string())
            .collect()
    }

    /// Tool names contributed by one extension, ordered by name.
    pub fn tool_names_of(&self, id: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .contributions_of(id)
            .iter()
            .filter_map(|contribution| match contribution {
                Contribution::Tool(tool) => Some(tool.name().to_string()),
                _ => None,
            })
            .collect();
        names.sort();
        names
    }

    /// One tool by name.
    pub fn tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools().into_iter().find(|tool| tool.name() == name)
    }

    /// Every hook contribution.
    pub fn hooks(&self) -> Vec<&HookContribution> {
        self.contributions()
            .into_iter()
            .filter_map(|(_, contribution)| match contribution {
                Contribution::Hook(hook) => Some(hook),
                _ => None,
            })
            .collect()
    }

    /// Every host command.
    pub fn commands(&self) -> Vec<&PluginCommand> {
        self.contributions()
            .into_iter()
            .filter_map(|(_, contribution)| match contribution {
                Contribution::Command(command) => Some(command),
                _ => None,
            })
            .collect()
    }

    /// Every skill.
    pub fn skills(&self) -> Vec<&SkillContribution> {
        self.contributions()
            .into_iter()
            .filter_map(|(_, contribution)| match contribution {
                Contribution::Skill(skill) => Some(skill),
                _ => None,
            })
            .collect()
    }

    /// Every skill directory.
    pub fn skill_dirs(&self) -> Vec<&SkillDirContribution> {
        self.contributions()
            .into_iter()
            .filter_map(|(_, contribution)| match contribution {
                Contribution::SkillDir(dir) => Some(dir),
                _ => None,
            })
            .collect()
    }

    /// Every UI panel.
    pub fn panels(&self) -> Vec<&UiPanel> {
        self.contributions()
            .into_iter()
            .filter_map(|(_, contribution)| match contribution {
                Contribution::Panel(panel) => Some(panel),
                _ => None,
            })
            .collect()
    }

    /// Every prompt.
    pub fn prompts(&self) -> Vec<&PromptContribution> {
        self.contributions()
            .into_iter()
            .filter_map(|(_, contribution)| match contribution {
                Contribution::Prompt(prompt) => Some(prompt),
                _ => None,
            })
            .collect()
    }

    /// Every model provider an installed package declares, with its owner.
    ///
    /// This is the seam a host reads to lower a package's LLM endpoints onto
    /// its own provider tree. It is deliberately separate from
    /// [`Self::apply_to_agent`]: a provider is not a tool or a hook, and the
    /// model layer is a host surface like commands and panels, so a caller that
    /// has no provider tree simply does not read it.
    pub fn providers(&self) -> Vec<(&Extension, &ProviderContribution)> {
        self.contributions()
            .into_iter()
            .filter_map(|(owner, contribution)| match contribution {
                Contribution::Provider(provider) => self
                    .extensions
                    .get(owner)
                    .map(|extension| (extension, provider)),
                _ => None,
            })
            .collect()
    }

    /// Every refusal recorded by an installed package.
    pub fn refusals(&self) -> &[Refusal] {
        &self.refusals
    }

    /// The number of registered extensions.
    pub fn len(&self) -> usize {
        self.extensions.len()
    }

    /// Whether no extension is registered.
    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }

    /// Install every tool, hook, and skill on `agent`.
    ///
    /// Tools go to the agent's own [`ToolRegistry`](crate::tools::ToolRegistry),
    /// hooks to the existing [`AgentPlugin`](crate::agent::plugin::AgentPlugin) registry, and skills through the
    /// agent's own [`SKILL.md`](crate::skills::SkillLoader) reader. Commands,
    /// panels, and prompts are host surfaces and are installed by
    /// [`ExtensionRegistry::install_host_items`] and [`ExtensionRegistry::prompts`]
    /// instead.
    pub fn apply_to_agent(&self, agent: &mut Agent) -> Result<AppliedExtensions, ExtensionError> {
        let existing: HashSet<String> = agent
            .registry()
            .map(|registry| registry.list().into_iter().collect())
            .unwrap_or_default();
        for (owner, contribution) in self.contributions() {
            if let Contribution::Tool(tool) = contribution {
                if existing.contains(tool.name()) {
                    return Err(ExtensionError::ToolConflict {
                        name: tool.name().to_string(),
                        extension: owner.to_string(),
                    });
                }
            }
        }

        let mut applied = AppliedExtensions::default();
        for (owner, contribution) in self.contributions() {
            match contribution {
                Contribution::Tool(tool) => {
                    agent
                        .try_add_tool(Arc::clone(tool))
                        .map_err(|error| ExtensionError::Tool {
                            name: tool.name().to_string(),
                            extension: owner.to_string(),
                            message: error.to_string(),
                        })?;
                    applied.tools.push(tool.name().to_string());
                }
                Contribution::Hook(hook) => {
                    agent.add_plugin(Arc::clone(&hook.plugin));
                    applied.hooks.push(hook.plugin.name().to_string());
                }
                Contribution::Skill(skill) => {
                    agent
                        .register_skill_manifest(skill.manifest.clone())
                        .map_err(|error| ExtensionError::Skill {
                            extension: owner.to_string(),
                            message: error.to_string(),
                        })?;
                    applied.skills.push(skill.name.clone());
                }
                Contribution::SkillDir(dir) => {
                    agent
                        .register_skills_from_dir(dir.path.clone())
                        .map_err(|error| ExtensionError::Skill {
                            extension: owner.to_string(),
                            message: error.to_string(),
                        })?;
                    applied.skill_dirs.push(dir.path.display().to_string());
                }
                // Host surfaces: commands, panels, prompts and model providers
                // are installed by the host's own registries, not on the agent's
                // tool registry.
                Contribution::Command(_)
                | Contribution::Panel(_)
                | Contribution::Prompt(_)
                | Contribution::Provider(_) => {}
            }
        }
        Ok(applied)
    }

    /// Register commands and panels in the existing host
    /// [`PluginRegistry`](crate::plugins::PluginRegistry).
    ///
    /// The adapter implements the existing [`Plugin`] trait, so the host
    /// registry's all-or-nothing install and its cross-kind collision check
    /// apply unchanged.
    pub fn install_host_items(
        &self,
        registry: &mut PluginRegistry,
    ) -> Result<HostInstall, ExtensionError> {
        let mut installed = HostInstall::default();
        for extension in self.extensions.values() {
            registry.install(&HostExtension {
                extension: extension.clone(),
            })?;
            for contribution in &extension.contributions {
                match contribution {
                    Contribution::Command(command) => {
                        installed.commands.push(command.name.clone());
                    }
                    Contribution::Panel(panel) => {
                        installed.panels.push(panel.id.clone());
                    }
                    _ => {}
                }
            }
        }
        Ok(installed)
    }
}

struct HostExtension {
    extension: Extension,
}

impl Plugin for HostExtension {
    // The host registry's Plugin::name is the owner id recorded against every
    // item; it is deliberately the package id, not the display name.
    #[allow(clippy::misnamed_getters)]
    fn name(&self) -> &str {
        &self.extension.id
    }

    fn register(&self, registry: &mut PluginRegistry) {
        for contribution in &self.extension.contributions {
            match contribution {
                Contribution::Command(command) => {
                    let _ = registry.register_command(self.name(), command.clone());
                }
                Contribution::Panel(panel) => {
                    let _ = registry.register_panel(self.name(), panel.clone());
                }
                _ => {}
            }
        }
    }
}
