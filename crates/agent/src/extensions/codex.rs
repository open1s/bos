//! The Codex package shim.
//!
//! A Codex plugin carries `.codex-plugin/plugin.json` with `name`,
//! `version`, `description`, `author`, `homepage`, `repository`,
//! `license`, `keywords`, `skills` (a skills directory), `mcpServers` (an
//! MCP file path), `apps` (an app file path), and `interface`
//! (`displayName`, `shortDescription`, `longDescription`, `developerName`,
//! `category`, `capabilities`, `websiteURL`, `privacyPolicyURL`,
//! `termsOfServiceURL`, `defaultPrompt`, `brandColor`, `composerIcon`,
//! `logo`, `logoDark`). Prompt commands live in `commands/*.md` with a
//! `description` frontmatter and `$ARGUMENTS` interpolation; skills live in
//! `skills/<name>/SKILL.md`. The declarative surface (`tools/*.json`,
//! `commands/*.md`, `skills/*/SKILL.md`, `agents/*.md`) is loaded. MCP servers
//! are recorded as refusals by this shim, which does not own processes; a host
//! with an MCP pool (BSH) reads the same `.mcp.json` and starts them. App
//! connectors remain external and are refused.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::declarative;
use super::{
    dedup_capabilities, manifest_error, manifest_string, read_json, resolve_package_path,
    Capability, Extension, ExtensionDetails, ExtensionError, ExtensionStandard, LoadedPackage,
    Refusal, RefusedFeature,
};

const STANDARD: ExtensionStandard = ExtensionStandard::Codex;

/// Load one Codex plugin directory.
pub fn load(root: &Path) -> Result<LoadedPackage, ExtensionError> {
    let manifest_path = root.join(".codex-plugin").join("plugin.json");
    if !manifest_path.is_file() {
        return Err(ExtensionError::WrongStandard {
            path: root.to_path_buf(),
            expected: STANDARD,
            found: ExtensionStandard::DeepSeekHarness,
        });
    }
    let value = read_json(&manifest_path, STANDARD)?;
    let object = value.as_object().ok_or_else(|| {
        manifest_error(
            STANDARD,
            &manifest_path,
            "plugin.json must be a JSON object",
        )
    })?;
    let id = manifest_string(object, "name", STANDARD, &manifest_path)?;
    let version = manifest_string(object, "version", STANDARD, &manifest_path)?;
    let package_description = object
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    let interface = object.get("interface").and_then(Value::as_object);
    let display_name = interface
        .and_then(|interface| interface.get("displayName"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| id.clone());
    let description = interface
        .and_then(|interface| interface.get("shortDescription"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(package_description);

    let mut details = ExtensionDetails::default();
    let icon = interface
        .and_then(|interface| {
            interface
                .get("logo")
                .or_else(|| interface.get("composerIcon"))
        })
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(raw) = icon {
        let path = resolve_package_path(root, &raw, STANDARD, &manifest_path)?;
        if path.is_file() {
            details.icon = Some(path);
        }
    }

    let mut capabilities = BTreeSet::new();
    let mut contributions = Vec::new();
    let mut refusals = Vec::new();

    let tools = declarative::read_tools(root, STANDARD)?;
    if !tools.is_empty() {
        capabilities.insert(Capability::Tools);
        contributions.extend(tools);
    }
    let commands = declarative::read_commands(root, STANDARD, &id)?;
    if !commands.is_empty() {
        capabilities.insert(Capability::Commands);
        capabilities.insert(Capability::Prompts);
        capabilities.insert(Capability::Tools);
        contributions.extend(commands);
    }
    let skills_root = match object.get("skills").and_then(Value::as_str) {
        Some(raw) => Some(resolve_package_path(root, raw, STANDARD, &manifest_path)?),
        None => {
            let conventional = root.join("skills");
            conventional.is_dir().then_some(conventional)
        }
    };
    if let Some(dir) = skills_root {
        let skills = declarative::read_skills(&dir, STANDARD)?;
        if !skills.is_empty() {
            capabilities.insert(Capability::Skills);
            contributions.extend(skills);
        }
    }
    let agents = declarative::read_agents(root, STANDARD)?;
    if !agents.is_empty() {
        capabilities.insert(Capability::Prompts);
        contributions.extend(agents);
    }
    // The same declarative model-provider carrier the DSH shim reads. A Codex
    // manifest has no provider field at all, so a package that reaches for one
    // does it through this file; nothing about the file is standard-specific.
    let providers = declarative::read_providers(root, STANDARD)?;
    if !providers.is_empty() {
        capabilities.insert(Capability::Models);
        contributions.extend(providers);
    }

    if let Some(mcp) = object.get("mcpServers") {
        let (mcp_path, servers) = mcp_servers(root, mcp, &manifest_path)?;
        if !servers.is_empty() {
            capabilities.insert(Capability::Tools);
        }
        for server in servers {
            refusals.push(Refusal::new(
                &id,
                RefusedFeature::McpServer,
                Some(mcp_path.clone()),
                format!(
                    "the plugin declares MCP server '{}', which needs an external MCP process this shim does not start",
                    server
                ),
            ));
        }
    }

    if let Some(apps) = object.get("apps") {
        let (apps_path, connectors) = app_connectors(root, apps, &manifest_path)?;
        for connector in connectors {
            refusals.push(Refusal::new(
                &id,
                RefusedFeature::ExternalApp,
                Some(apps_path.clone()),
                format!(
                    "the plugin declares external app connector '{}', which this shim does not connect",
                    connector
                ),
            ));
        }
    }

    let mut extension = Extension::new(id, display_name, version, STANDARD, root)
        .with_manifest(manifest_path)
        .with_capabilities(dedup_capabilities(capabilities))
        .with_details(details)
        .with_contributions(contributions);
    if let Some(description) = description {
        extension = extension.with_description(description);
    }
    Ok(LoadedPackage::new(extension, refusals))
}

fn mcp_servers(
    root: &Path,
    value: &Value,
    manifest: &Path,
) -> Result<(PathBuf, Vec<String>), ExtensionError> {
    match value {
        Value::String(raw) => {
            let path = resolve_package_path(root, raw, STANDARD, manifest)?;
            if !path.is_file() {
                return Err(manifest_error(
                    STANDARD,
                    manifest,
                    format!("mcpServers '{}' does not exist", path.display()),
                ));
            }
            let value = read_json(&path, STANDARD)?;
            Ok((path.clone(), named_servers(&value, &path, "mcpServers")?))
        }
        Value::Object(map) => Ok((manifest.to_path_buf(), map.keys().cloned().collect())),
        _ => Err(manifest_error(
            STANDARD,
            manifest,
            "mcpServers must be a path string or an object",
        )),
    }
}

fn app_connectors(
    root: &Path,
    value: &Value,
    manifest: &Path,
) -> Result<(PathBuf, Vec<String>), ExtensionError> {
    match value {
        Value::String(raw) => {
            let path = resolve_package_path(root, raw, STANDARD, manifest)?;
            if !path.is_file() {
                return Err(manifest_error(
                    STANDARD,
                    manifest,
                    format!("apps '{}' does not exist", path.display()),
                ));
            }
            let value = read_json(&path, STANDARD)?;
            Ok((path.clone(), named_servers(&value, &path, "apps")?))
        }
        Value::Object(map) => Ok((manifest.to_path_buf(), map.keys().cloned().collect())),
        _ => Err(manifest_error(
            STANDARD,
            manifest,
            "apps must be a path string or an object",
        )),
    }
}

fn named_servers(value: &Value, path: &Path, key: &str) -> Result<Vec<String>, ExtensionError> {
    let servers = value
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| manifest_error(STANDARD, path, format!("the file has no `{key}` object")))?;
    Ok(servers.keys().cloned().collect())
}
