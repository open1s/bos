//! The declarative runtime: contributions that are files, not code.
//!
//! A declarative package can ship tools whose bodies are pure transformations,
//! prompt commands, prompt and context fragments, and a declarative panel. It
//! executes nothing, so it needs no process and no wasm instance; the security
//! boundary is the file parser and the UI validator.
//!
//! Every declared contribution must have its file. A package that declares a
//! tool and ships no descriptor is refused rather than silently contributing
//! less than it said, because the manifest is the standard's contract.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{
    load_tool_descriptors, CallContext, Implementation, Runtime, ToolDescriptor, ToolFile,
};
use crate::extension::manifest::{validate_relative_path, Manifest};
use crate::extension::types::{ContributionId, ExtensionError, ExtensionId, RuntimeKind};
use crate::extension::ui::{PanelLocation, PanelVisibility, UiDocument, UiSession};

/// The largest declarative contribution file.
pub(crate) const MAX_CONTRIBUTION_BYTES: u64 = 256 * 1024;

/// One tool plus its pure body.
#[derive(Debug, Clone)]
pub(crate) struct DeclarativeTool {
    /// The model-facing descriptor.
    pub(crate) descriptor: ToolDescriptor,
    /// The pure body.
    pub(crate) implementation: Implementation,
}

/// A prompt command from a Markdown file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandContribution {
    /// Contribution id.
    pub(crate) id: ContributionId,
    /// Description shown in the command list.
    pub(crate) description: String,
    /// Body template.
    pub(crate) template: String,
}

/// A prompt or context fragment from a Markdown file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextContribution {
    /// Contribution id.
    pub(crate) id: ContributionId,
    /// One-line description.
    pub(crate) description: String,
    /// Body text.
    pub(crate) text: String,
}

/// A declarative panel.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PanelContribution {
    /// Contribution id.
    pub(crate) id: ContributionId,
    /// Panel title.
    pub(crate) title: String,
    /// Optional package-relative icon path.
    pub(crate) icon: Option<String>,
    /// Where the host mounts it.
    pub(crate) location: PanelLocation,
    /// Whether it starts visible.
    pub(crate) visibility: PanelVisibility,
    /// The validated view.
    pub(crate) view: UiDocument,
}

/// A raw file contribution the host records but does not execute yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawContribution {
    /// Contribution id.
    pub(crate) id: ContributionId,
    /// Package-relative source path.
    pub(crate) path: PathBuf,
}

/// Everything a declarative package contributes beyond its tools.
#[derive(Debug, Clone, Default)]
pub(crate) struct DeclarativeContributions {
    /// Prompt commands.
    pub(crate) commands: Vec<CommandContribution>,
    /// Prompt fragments.
    pub(crate) prompts: Vec<TextContribution>,
    /// Context fragments.
    pub(crate) contexts: Vec<TextContribution>,
    /// Panels.
    pub(crate) panels: Vec<PanelContribution>,
    /// Workflows, recorded but not executed.
    pub(crate) workflows: Vec<RawContribution>,
    /// Themes, recorded but not applied.
    pub(crate) themes: Vec<RawContribution>,
}

impl DeclarativeContributions {
    /// Whether any contribution would reach a host surface.
    pub(crate) fn is_empty(&self) -> bool {
        self.commands.is_empty()
            && self.prompts.is_empty()
            && self.contexts.is_empty()
            && self.panels.is_empty()
            && self.workflows.is_empty()
            && self.themes.is_empty()
    }
}

/// The declarative runtime.
#[derive(Debug)]
pub(crate) struct DeclarativeRuntime {
    extension: ExtensionId,
    tools: Vec<DeclarativeTool>,
    contributions: DeclarativeContributions,
    max_output_bytes: u64,
}

impl DeclarativeRuntime {
    /// Load and validate every declared file.
    pub(crate) fn new(root: &Path, manifest: &Manifest) -> Result<Self, ExtensionError> {
        let descriptors = load_tool_descriptors(root, manifest)?;
        let mut implementations = BTreeMap::new();
        for descriptor in &descriptors {
            let path = root.join(format!("tools/{}.json", descriptor.id.as_str()));
            let file = read_tool_file(&path)?;
            let implementation = file
                .implementation
                .ok_or_else(|| ExtensionError::Manifest {
                    path: path.clone(),
                    message: format!("declarative tool {} needs an implementation", descriptor.id),
                })?;
            implementations.insert(descriptor.id.clone(), implementation);
        }
        let tools: Vec<DeclarativeTool> = descriptors
            .into_iter()
            .map(|descriptor| {
                let implementation = implementations
                    .remove(&descriptor.id)
                    .expect("every descriptor had an implementation");
                DeclarativeTool {
                    descriptor,
                    implementation,
                }
            })
            .collect();
        let contributions = load_contributions(root, manifest)?;
        Ok(Self {
            extension: manifest.id().clone(),
            tools,
            contributions,
            max_output_bytes: manifest.resources.output_bytes(),
        })
    }

    /// The non-tool contributions.
    pub(crate) fn contributions(&self) -> &DeclarativeContributions {
        &self.contributions
    }

    /// The tool descriptors, without going through the async trait.
    pub(crate) fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools
            .iter()
            .map(|tool| tool.descriptor.clone())
            .collect()
    }
}

#[async_trait]
impl Runtime for DeclarativeRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Declarative
    }

    async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        Ok(self
            .tools
            .iter()
            .map(|tool| tool.descriptor.clone())
            .collect())
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        _context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        let Some(tool) = self
            .tools
            .iter()
            .find(|tool| tool.descriptor.id.as_str() == name)
        else {
            return Err(ExtensionError::NotFound {
                id: format!("{}::{name}", self.extension),
            });
        };
        tool.implementation
            .run(&arguments, self.max_output_bytes, &self.extension)
    }
}

fn read_tool_file(path: &Path) -> Result<ToolFile, ExtensionError> {
    let metadata = std::fs::metadata(path).map_err(|source| ExtensionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > MAX_CONTRIBUTION_BYTES {
        return Err(ExtensionError::Limit {
            extension: path.display().to_string(),
            message: format!("tool descriptor is larger than {MAX_CONTRIBUTION_BYTES} bytes"),
        });
    }
    let text = std::fs::read_to_string(path).map_err(|source| ExtensionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|error| ExtensionError::Json {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn read_text(path: &Path) -> Result<String, ExtensionError> {
    let metadata = std::fs::metadata(path).map_err(|source| ExtensionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > MAX_CONTRIBUTION_BYTES {
        return Err(ExtensionError::Limit {
            extension: path.display().to_string(),
            message: format!("contribution is larger than {MAX_CONTRIBUTION_BYTES} bytes"),
        });
    }
    std::fs::read_to_string(path).map_err(|source| ExtensionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn contribution_path(
    root: &Path,
    directory: &str,
    id: &ContributionId,
) -> Result<PathBuf, ExtensionError> {
    let relative = format!("{directory}/{}.md", id.as_str());
    let relative =
        validate_relative_path(&relative).map_err(|reason| ExtensionError::Manifest {
            path: root.join("extension.toml"),
            message: format!("contribution {id}: {reason}"),
        })?;
    Ok(root.join(relative))
}

pub(crate) fn load_contributions(
    root: &Path,
    manifest: &Manifest,
) -> Result<DeclarativeContributions, ExtensionError> {
    let mut contributions = DeclarativeContributions::default();
    for id in &manifest.contributes.commands {
        let path = contribution_path(root, "commands", id)?;
        let text = read_text(&path)?;
        let (description, template) = parse_markdown(&text, id);
        contributions.commands.push(CommandContribution {
            id: id.clone(),
            description,
            template,
        });
    }
    for id in &manifest.contributes.prompts {
        let path = contribution_path(root, "prompts", id)?;
        contributions
            .prompts
            .push(text_contribution(root, id, &path)?);
    }
    for id in &manifest.contributes.contexts {
        let path = contribution_path(root, "contexts", id)?;
        contributions
            .contexts
            .push(text_contribution(root, id, &path)?);
    }
    for id in &manifest.contributes.panels {
        let relative = format!("panels/{}.json", id.as_str());
        let path = root.join(validate_relative_path(&relative).map_err(|reason| {
            ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: format!("panel {id}: {reason}"),
            }
        })?);
        let text = read_text(&path)?;
        let file: PanelFile =
            serde_json::from_str(&text).map_err(|error| ExtensionError::Json {
                path: path.clone(),
                message: error.to_string(),
            })?;
        if file.title.trim().is_empty() || file.title.chars().count() > 120 {
            return Err(ExtensionError::Manifest {
                path,
                message: "panel title must be 1..=120 characters".to_string(),
            });
        }
        if let Some(icon) = &file.icon {
            if validate_relative_path(icon).is_err() {
                return Err(ExtensionError::Manifest {
                    path,
                    message: "panel icon must be a package-relative path".to_string(),
                });
            }
        }
        let session = UiSession::open(manifest.id().clone(), file.view)?;
        contributions.panels.push(PanelContribution {
            id: id.clone(),
            title: file.title,
            icon: file.icon,
            location: file.location,
            visibility: file.visibility,
            view: session.document().clone(),
        });
    }
    for id in &manifest.contributes.workflows {
        contributions
            .workflows
            .push(raw_contribution(root, "workflows", "yaml", id)?);
    }
    for id in &manifest.contributes.themes {
        contributions
            .themes
            .push(raw_contribution(root, "themes", "json", id)?);
    }
    Ok(contributions)
}

fn raw_contribution(
    root: &Path,
    directory: &str,
    extension: &str,
    id: &ContributionId,
) -> Result<RawContribution, ExtensionError> {
    let relative = format!("{directory}/{}.{extension}", id.as_str());
    let path = root.join(validate_relative_path(&relative).map_err(|reason| {
        ExtensionError::Manifest {
            path: root.join("extension.toml"),
            message: format!("contribution {id}: {reason}"),
        }
    })?);
    if !path.is_file() {
        return Err(ExtensionError::Io {
            path,
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "declared contribution file is missing",
            ),
        });
    }
    Ok(RawContribution {
        id: id.clone(),
        path,
    })
}

fn text_contribution(
    root: &Path,
    id: &ContributionId,
    path: &Path,
) -> Result<TextContribution, ExtensionError> {
    let text = read_text(path)?;
    let (description, body) = parse_markdown(&text, id);
    let _ = root;
    Ok(TextContribution {
        id: id.clone(),
        description,
        text: body,
    })
}

/// Split optional YAML frontmatter from a Markdown contribution.
///
/// Only the two keys this standard defines are read: description and name.
/// Unknown keys are ignored so a shared frontmatter vocabulary does not make a
/// package unreadable.
fn parse_markdown(text: &str, id: &ContributionId) -> (String, String) {
    let trimmed = text.trim_start_matches('\u{feff}');
    let mut description = None;
    let mut body = trimmed;
    if let Some(rest) = trimmed.strip_prefix("---") {
        let rest = rest
            .strip_prefix("\r\n")
            .or_else(|| rest.strip_prefix('\n'))
            .unwrap_or(rest);
        if let Some(end) = rest.find("\n---") {
            let frontmatter = &rest[..end];
            for line in frontmatter.lines() {
                if let Some(value) = line.strip_prefix("description:") {
                    description = Some(value.trim().trim_matches('"').to_string());
                } else if let Some(value) = line.strip_prefix("name:") {
                    if description.is_none() {
                        description = Some(value.trim().trim_matches('"').to_string());
                    }
                }
            }
            let after = &rest[end + 4..];
            body = after
                .strip_prefix("\r\n")
                .or_else(|| after.strip_prefix('\n'))
                .unwrap_or(after);
        }
    }
    let body = body.trim().to_string();
    let description = description
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| first_line_summary(&body, id));
    (description, body)
}

fn first_line_summary(body: &str, id: &ContributionId) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(id.as_str());
    let line = line.trim_start_matches('#').trim();
    let mut summary: String = line.chars().take(200).collect();
    if summary.is_empty() {
        summary = id.as_str().to_string();
    }
    summary
}

/// One panel file.
#[derive(Debug, Deserialize)]
struct PanelFile {
    title: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    location: PanelLocation,
    #[serde(default, rename = "default_visibility")]
    visibility: PanelVisibility,
    view: UiDocument,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::ui::UI_SCHEMA_VERSION;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let index = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "bsh-ext-declarative-{label}-{}-{index}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            for directory in ["tools", "commands", "prompts", "contexts", "panels"] {
                std::fs::create_dir_all(path.join(directory)).expect("scratch");
            }
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn manifest() -> Manifest {
        Manifest::parse(
            r#"
schema_version = "1.0"

[extension]
id = "com.example.declarative"
name = "Declarative"
version = "1.0.0"
description = "A declarative package"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[contributes]
tools = ["echo", "greet"]
commands = ["refresh"]
prompts = ["system"]
contexts = ["workspace"]
panels = ["status"]

[resources]
max_output_bytes = 4096
"#,
            Path::new("extension.toml"),
        )
        .expect("manifest")
    }

    fn write_fixture(scratch: &Scratch) {
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","implementation":{"kind":"echo_arguments"}}"#,
        )
        .expect("echo");
        std::fs::write(
            scratch.0.join("tools/greet.json"),
            r#"{"description":"Greet","implementation":{"kind":"template","text":"hello {{name}}"}}"#,
        )
        .expect("greet");
        std::fs::write(
            scratch.0.join("commands/refresh.md"),
            "---\ndescription: Refresh the tree\n---\nRun the refresh.\n",
        )
        .expect("command");
        std::fs::write(scratch.0.join("prompts/system.md"), "# System\nBe brief.\n")
            .expect("prompt");
        std::fs::write(
            scratch.0.join("contexts/workspace.md"),
            "Workspace facts.\n",
        )
        .expect("context");
        std::fs::write(
            scratch.0.join("panels/status.json"),
            format!(
                r#"{{"title":"Status","view":{{"schema_version":"{UI_SCHEMA_VERSION}","root":{{"type":"text","text":"Ready"}}}}}}"#
            ),
        )
        .expect("panel");
    }

    #[test]
    fn a_package_loads_every_declared_contribution() {
        let scratch = Scratch::new("load");
        write_fixture(&scratch);
        let runtime = DeclarativeRuntime::new(&scratch.0, &manifest()).expect("runtime");
        let contributions = runtime.contributions();
        assert_eq!(contributions.commands.len(), 1);
        assert_eq!(contributions.commands[0].description, "Refresh the tree");
        assert_eq!(contributions.prompts[0].description, "System");
        assert_eq!(contributions.contexts[0].text, "Workspace facts.");
        assert_eq!(contributions.panels[0].title, "Status");
        assert_eq!(
            contributions.panels[0].view.root,
            UiDocument {
                schema_version: UI_SCHEMA_VERSION.to_string(),
                root: crate::extension::ui::UiNode::Text {
                    text: "Ready".to_string()
                },
            }
            .root
        );
    }

    #[tokio::test]
    async fn declarative_tools_run_pure_bodies() {
        let scratch = Scratch::new("call");
        write_fixture(&scratch);
        let runtime = DeclarativeRuntime::new(&scratch.0, &manifest()).expect("runtime");
        let context = CallContext::new(manifest().id().clone(), "correlation", None, 1000);
        let echoed = runtime
            .call_tool("echo", serde_json::json!({"x": 1}), &context)
            .await
            .expect("echo");
        assert_eq!(echoed["x"], 1);
        let greeted = runtime
            .call_tool("greet", serde_json::json!({"name": "world"}), &context)
            .await
            .expect("greet");
        assert_eq!(greeted, "hello world");
        assert!(runtime
            .call_tool("missing", serde_json::json!({}), &context)
            .await
            .is_err());
    }

    #[test]
    fn a_declared_tool_without_a_body_is_refused() {
        let scratch = Scratch::new("nobody");
        write_fixture(&scratch);
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo"}"#,
        )
        .expect("echo");
        let error = DeclarativeRuntime::new(&scratch.0, &manifest()).expect_err("no body");
        assert!(error.to_string().contains("implementation"), "{error}");
    }

    #[test]
    fn a_missing_declared_file_is_refused() {
        let scratch = Scratch::new("missing");
        write_fixture(&scratch);
        std::fs::remove_file(scratch.0.join("panels/status.json")).expect("remove");
        assert!(DeclarativeRuntime::new(&scratch.0, &manifest()).is_err());
    }

    #[test]
    fn frontmatter_is_read_and_unknown_keys_are_ignored() {
        let id = ContributionId::parse("refresh").expect("id");
        let (description, body) = parse_markdown(
            "---\ndescription: Refresh\nowner: someone\n---\nBody\n",
            &id,
        );
        assert_eq!(description, "Refresh");
        assert_eq!(body, "Body");
        let (description, body) = parse_markdown("# Title\n\nBody", &id);
        assert_eq!(description, "Title");
        assert_eq!(body, "# Title\n\nBody");
    }
}
