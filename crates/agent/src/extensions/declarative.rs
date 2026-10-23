//! The declarative package surface both shims read.
//!
//! Neither standard declares tools as JSON Schema in its manifest. A package
//! may therefore ship the following files, which this module reads without
//! executing anything:
//!
//! - `tools/*.json`: one tool per file, with a JSON Schema and a pure body
//!   (`echo_arguments`, `constant`, or `template`);
//! - `commands/*.md`: a prompt command, with a `description` frontmatter and
//!   `$ARGUMENTS` interpolation. Each command yields a host command, a prompt,
//!   and a callable prompt tool;
//! - `skills/<name>/SKILL.md`: a skill, read through the agent crate's own
//!   frontmatter reader;
//! - `agents/*.md`: a prompt (a system-prompt fragment), not a tool.
//!
//! These files are explicitly BOS-owned: the DeepSeek Harness reference
//! registers tools, commands, and skills imperatively from JavaScript, and the
//! Codex reference has no `tools` field at all. A package that only carries
//! the imperative forms is refused by its shim; a package that also ships these
//! declarative files gets those contributions.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::plugins::PluginCommand;
use crate::tools::{Tool, ToolError};

use super::{
    manifest_error, read_text, Contribution, ExtensionError, ExtensionStandard, PromptContribution,
    ProviderContribution, ProviderModel, SkillContribution,
};

/// Replace every character of `raw` that is not ASCII alphanumeric with `_`.
fn sanitize_name(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "extension".to_string()
    } else {
        trimmed
    }
}

/// The prefix a package gives the tools it derives from prompt commands.
///
/// ``local/dsh-echo` becomes `dsh_echo` and `vercel` stays `vercel`, so two
/// packages can both ship a `deploy.md` command without colliding.
pub(crate) fn tool_prefix(extension: &str) -> String {
    let base = extension.rsplit('/').next().unwrap_or(extension);
    sanitize_name(base)
}

fn json_files(dir: &Path) -> Result<Vec<PathBuf>, ExtensionError> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|source| ExtensionError::Io {
        path: dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ExtensionError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Every Markdown file directly under `dir`, `*.md` and `*.md.tmpl`.
///
/// Both shapes are shipped: the Codex plugin `commands/` and `agents/`
/// directories on this machine carry 12 plain `.md` files and an equal number
/// of `.md.tmpl` twins (Vercel, Zoom and Figma all ship the pair), where the
/// template is the same document with its placeholders unsubstituted. Reading
/// only `.md` would drop half of a package's commands silently.
///
/// A `_`-prefixed file is a shared fragment, not a command: `vercel` and `zoom`
/// both ship `_conventions.md` for other command files to reference, and
/// registering it would put a private helper in the reader's command list.
fn markdown_files(dir: &Path) -> Result<Vec<PathBuf>, ExtensionError> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|source| ExtensionError::Io {
        path: dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ExtensionError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_file() && is_markdown(&path) && !is_fragment(&path) {
            files.push(path);
        }
    }
    // A resolved `.md` is ordered before its `.md.tmpl` twin, so the pair's
    // deduplication keeps the document a reader would actually run. Lexical order
    // happens to give this (`.md` is a prefix of `.md.tmpl`), but the rule is
    // stated explicitly because it is a contract, not a coincidence.
    files.sort_by(|left, right| {
        let key = |path: &PathBuf| (document_name(path), is_template(path));
        key(left).cmp(&key(right))
    });
    Ok(files)
}

/// Whether a file is the packer's unsubstituted template variant.
fn is_template(path: &Path) -> bool {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".md.tmpl"))
}

/// Whether a file is a Markdown document this shim reads: `x.md` or `x.md.tmpl`.
fn is_markdown(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default();
    let lowered = name.to_ascii_lowercase();
    lowered.ends_with(".md") || lowered.ends_with(".md.tmpl")
}

/// Whether a Markdown file is a shared fragment rather than a contribution.
fn is_fragment(path: &Path) -> bool {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.starts_with('_'))
}

/// The contribution name a Markdown file yields.
///
/// `deploy.md` and `deploy.md.tmpl` are the same command, so both reduce to
/// `deploy`; a `.tmpl` suffix is the packer's marker, never part of the name.
fn document_name(path: &Path) -> String {
    let stem = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default();
    let lowered = stem.to_ascii_lowercase();
    let trimmed = if lowered.ends_with(".md.tmpl") {
        &stem[..stem.len() - ".md.tmpl".len()]
    } else if lowered.ends_with(".md") {
        &stem[..stem.len() - ".md".len()]
    } else {
        stem
    };
    trimmed.to_string()
}
fn frontmatter_str(frontmatter: Option<&serde_yaml::Value>, key: &str) -> Option<String> {
    let mapping = frontmatter?.as_mapping()?;
    let key = serde_yaml::Value::String(key.to_string());
    mapping
        .get(&key)
        .and_then(serde_yaml::Value::as_str)
        .map(str::to_string)
}

/// Split a Markdown file into its YAML frontmatter and its body.
fn split_frontmatter(text: &str) -> Result<(Option<serde_yaml::Value>, String), String> {
    let trimmed = text.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---") {
        return Ok((None, text.trim().to_string()));
    }
    let after = &trimmed[3..];
    let after = after
        .strip_prefix("\r\n")
        .or_else(|| after.strip_prefix('\n'))
        .unwrap_or(after);
    let Some(end) = after.find("\n---") else {
        return Err("frontmatter starts with --- but has no closing ---".to_string());
    };
    let yaml = &after[..end];
    let mut body = &after[end + 4..];
    body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'))
        .unwrap_or(body);
    let value: serde_yaml::Value = serde_yaml::from_str(yaml)
        .map_err(|error| format!("frontmatter is not valid YAML: {error}"))?;
    Ok((Some(value), body.trim().to_string()))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolDescriptor {
    name: String,
    description: String,
    #[serde(default = "default_parameters")]
    parameters: Value,
    #[serde(default)]
    behavior: Behavior,
}

fn default_parameters() -> Value {
    json!({ "type": "object", "properties": {} })
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Behavior {
    #[default]
    EchoArguments,
    Constant {
        value: Value,
    },
    Template {
        template: String,
    },
}

/// A tool backed by one declarative descriptor.
struct DeclarativeTool {
    name: String,
    description: String,
    schema: Value,
    behavior: Behavior,
}

impl Tool for DeclarativeTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn category(&self) -> String {
        "extension".to_string()
    }

    fn json_schema(&self) -> Value {
        self.schema.clone()
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        match &self.behavior {
            Behavior::EchoArguments => Ok(input.clone()),
            Behavior::Constant { value } => Ok(value.clone()),
            Behavior::Template { template } => render_template(template, input),
        }
    }
}

fn render_template(template: &str, input: &Value) -> Result<Value, ToolError> {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err(ToolError::InvalidInput(
                "unterminated template placeholder".to_string(),
            ));
        };
        let key = after[..end].trim();
        let value = input
            .get(key)
            .ok_or_else(|| ToolError::InvalidInput(format!("missing template argument: {key}")))?;
        let text = value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        output.push_str(&text);
        rest = &after[end + 2..];
    }
    output.push_str(rest);
    Ok(Value::String(output))
}

/// A tool that renders one `commands/*.md` prompt with its arguments.
struct PromptTool {
    name: String,
    description: String,
    body: String,
}

impl Tool for PromptTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn category(&self) -> String {
        "extension".to_string()
    }

    fn json_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "arguments": {
                    "type": "string",
                    "description": "Arguments for the prompt template."
                }
            }
        })
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let arguments = input.get("arguments").and_then(Value::as_str).unwrap_or("");
        let rendered = if self.body.contains("$ARGUMENTS") {
            self.body.replace("$ARGUMENTS", arguments)
        } else if arguments.is_empty() {
            self.body.clone()
        } else {
            format!("{}\n\nArguments: {}", self.body, arguments)
        };
        Ok(json!({ "prompt": rendered }))
    }
}

/// Read `tools/*.json` under `root`.
pub(crate) fn read_tools(
    root: &Path,
    standard: ExtensionStandard,
) -> Result<Vec<Contribution>, ExtensionError> {
    let dir = root.join("tools");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut contributions = Vec::new();
    for path in json_files(&dir)? {
        let text = read_text(&path)?;
        let descriptor: ToolDescriptor = serde_json::from_str(&text)
            .map_err(|error| manifest_error(standard, &path, error.to_string()))?;
        if descriptor.name.trim().is_empty() {
            return Err(manifest_error(
                standard,
                &path,
                "a tool descriptor needs a non-empty `name`",
            ));
        }
        if !descriptor.parameters.is_object() {
            return Err(manifest_error(
                standard,
                &path,
                "a tool descriptor's `parameters` must be a JSON Schema object",
            ));
        }
        contributions.push(Contribution::Tool(Arc::new(DeclarativeTool {
            name: descriptor.name,
            description: descriptor.description,
            schema: descriptor.parameters,
            behavior: descriptor.behavior,
        })));
    }
    Ok(contributions)
}

/// Read `commands/*.md` under `root`.
///
/// A packer ships `deploy.md` and `deploy.md.tmpl` as a PAIR - the second is the
/// same document with its placeholders unsubstituted - so both reduce to one
/// command. The plain `.md` wins when both exist (it is the resolved document),
/// and the `.tmpl` twin is skipped rather than registered twice: a duplicate is
/// a hard error in the registry, so without this rule a perfectly ordinary
/// packed plugin would fail to install at all.
pub(crate) fn read_commands(
    root: &Path,
    standard: ExtensionStandard,
    extension: &str,
) -> Result<Vec<Contribution>, ExtensionError> {
    let dir = root.join("commands");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut contributions = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for path in markdown_files(&dir)? {
        let name = document_name(&path);
        if !seen.insert(name.clone()) {
            // A resolved document already claimed this name; the template is the
            // same command.
            continue;
        }
        let text = read_text(&path)?;
        let (frontmatter, body) =
            split_frontmatter(&text).map_err(|message| manifest_error(standard, &path, message))?;
        if name.trim().is_empty() {
            return Err(manifest_error(
                standard,
                &path,
                "a command file needs a non-empty file name",
            ));
        }
        let description = frontmatter_str(frontmatter.as_ref(), "description").unwrap_or_default();
        let tool_name = format!("{}__{}", tool_prefix(extension), sanitize_name(&name));
        contributions.push(Contribution::Command(PluginCommand::new(
            name.clone(),
            description.clone(),
        )));
        contributions.push(Contribution::Prompt(PromptContribution {
            name: name.clone(),
            description: description.clone(),
            body: body.clone(),
            source: path.clone(),
            arguments_hint: body
                .contains("$ARGUMENTS")
                .then(|| "$ARGUMENTS".to_string()),
        }));
        contributions.push(Contribution::Tool(Arc::new(PromptTool {
            name: tool_name,
            description,
            body,
        })));
    }
    Ok(contributions)
}

/// Read `skills/<name>/SKILL.md` under `dir`.
pub(crate) fn read_skills(
    dir: &Path,
    standard: ExtensionStandard,
) -> Result<Vec<Contribution>, ExtensionError> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut skill_dirs = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|source| ExtensionError::Io {
        path: dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ExtensionError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() && path.join("SKILL.md").is_file() {
            skill_dirs.push(path);
        }
    }
    skill_dirs.sort();
    let mut contributions = Vec::new();
    for skill_dir in skill_dirs {
        let manifest = skill_dir.join("SKILL.md");
        let text = read_text(&manifest)?;
        let (frontmatter, _) = split_frontmatter(&text)
            .map_err(|message| manifest_error(standard, &manifest, message))?;
        let name = frontmatter_str(frontmatter.as_ref(), "name")
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| {
                manifest_error(
                    standard,
                    &manifest,
                    "SKILL.md frontmatter needs a non-empty `name`",
                )
            })?;
        let description = frontmatter_str(frontmatter.as_ref(), "description").unwrap_or_default();
        contributions.push(Contribution::Skill(SkillContribution {
            name,
            description,
            manifest,
        }));
    }
    Ok(contributions)
}

/// What a `providers/*.json` descriptor may declare.
///
/// Every field is optional but the identity ones, so a package can ship a bare
/// endpoint and let the host fill the rest, the same way a tool descriptor's
/// optional fields work.
#[derive(Debug, Deserialize)]
struct ProviderDescriptor {
    /// The provider id: registry key, router prefix, credential lookup key.
    id: String,
    /// Display name.
    #[serde(default)]
    name: Option<String>,
    /// The API base URL.
    #[serde(default, alias = "baseUrl")]
    base_url: Option<String>,
    /// The wire protocol; the host's default applies when absent.
    #[serde(default, rename = "apiMode", alias = "api_mode")]
    api_mode: Option<String>,
    /// Whether the endpoint needs a credential. Absent means it does, which is
    /// the safe reading: an endpoint that turns out to be keyless fails loudly
    /// against the vault rather than sending a request with no Authorization.
    #[serde(
        default = "default_true",
        rename = "requiresKey",
        alias = "requires_key"
    )]
    requires_key: bool,
    /// The models the provider offers.
    #[serde(default)]
    models: Vec<ProviderModelDescriptor>,
}

fn default_true() -> bool {
    true
}

/// What one entry of a provider descriptor's `models` array may declare.
#[derive(Debug, Deserialize)]
struct ProviderModelDescriptor {
    /// The id the API is called with.
    id: String,
    /// Display name.
    #[serde(default)]
    name: Option<String>,
    /// Output cap for this model.
    #[serde(default, rename = "maxTokens", alias = "max_tokens")]
    max_tokens: Option<u32>,
    /// Declared input types.
    #[serde(default, rename = "inputTypes", alias = "input_types")]
    input_types: Option<Vec<String>>,
    /// Reasoning effort.
    #[serde(default, rename = "reasoningEffort", alias = "reasoning_effort")]
    reasoning_effort: Option<String>,
}

/// Read `providers/*.json` under `root`.
///
/// This is the declarative carrier for a package that contributes an LLM
/// endpoint. The DeepSeek Harness reference registers one imperatively with
/// `ctx.llm.registerAdapter([id], adapter)`; a package that ALSO ships this
/// file gets a real provider here, and the imperative entry stays refused.
///
/// The base URL must be HTTP(S). A package cannot point the host's turns at a
/// `file:` or `javascript:` endpoint through this surface.
pub(crate) fn read_providers(
    root: &Path,
    standard: ExtensionStandard,
) -> Result<Vec<Contribution>, ExtensionError> {
    let dir = root.join("providers");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut contributions = Vec::new();
    for path in json_files(&dir)? {
        let text = read_text(&path)?;
        let descriptor: ProviderDescriptor = serde_json::from_str(&text)
            .map_err(|error| manifest_error(standard, &path, error.to_string()))?;
        if descriptor.id.trim().is_empty() {
            return Err(manifest_error(
                standard,
                &path,
                "a provider descriptor needs a non-empty `id`",
            ));
        }
        let base_url = descriptor
            .base_url
            .clone()
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty())
            .ok_or_else(|| {
                manifest_error(
                    standard,
                    &path,
                    "a provider descriptor needs a non-empty `base_url`",
                )
            })?;
        if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            return Err(manifest_error(
                standard,
                &path,
                format!("the base URL '{base_url}' must be an http or https address"),
            ));
        }
        if descriptor.models.is_empty() {
            return Err(manifest_error(
                standard,
                &path,
                "a provider descriptor needs at least one entry in `models`",
            ));
        }
        let mut models = Vec::with_capacity(descriptor.models.len());
        for model in descriptor.models {
            if model.id.trim().is_empty() {
                return Err(manifest_error(
                    standard,
                    &path,
                    "every provider model needs a non-empty `id`",
                ));
            }
            models.push(ProviderModel {
                id: model.id.trim().to_string(),
                name: model.name,
                max_tokens: model.max_tokens,
                input_types: model.input_types,
                reasoning_effort: model.reasoning_effort,
            });
        }
        contributions.push(Contribution::Provider(ProviderContribution {
            id: descriptor.id.trim().to_string(),
            name: descriptor.name,
            base_url,
            api_mode: descriptor.api_mode,
            requires_key: descriptor.requires_key,
            models,
        }));
    }
    Ok(contributions)
}

/// Read `agents/*.md` under `root` as prompt contributions.
pub(crate) fn read_agents(
    root: &Path,
    standard: ExtensionStandard,
) -> Result<Vec<Contribution>, ExtensionError> {
    let dir = root.join("agents");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut contributions = Vec::new();
    for path in markdown_files(&dir)? {
        let text = read_text(&path)?;
        let (frontmatter, body) =
            split_frontmatter(&text).map_err(|message| manifest_error(standard, &path, message))?;
        let name = frontmatter_str(frontmatter.as_ref(), "name")
            .filter(|name| !name.trim().is_empty())
            .or_else(|| {
                path.file_stem()
                    .and_then(std::ffi::OsStr::to_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "agent".to_string());
        let description = frontmatter_str(frontmatter.as_ref(), "description").unwrap_or_default();
        contributions.push(Contribution::Prompt(PromptContribution {
            name,
            description,
            body,
            source: path.clone(),
            arguments_hint: None,
        }));
    }
    Ok(contributions)
}
