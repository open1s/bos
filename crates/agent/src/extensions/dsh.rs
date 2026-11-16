//! The DeepSeek Harness package shim.
//!
//! The real manifest fields are `name`, `version`, and the `dsh` object:
//! `dsh.manifestVersion` (format `1`), `dsh.bundle.patch` (a
//! `cordis.patch.yml` path or an ordered list), and `dsh.client`
//! (`platform`, `immediately`, `inject`, `external`). The host entry
//! exports `apply(ctx, config)` and registers tools, commands, skills, and
//! hooks through the JavaScript host API. This shim parses the declarative
//! fields, loads the BOS declarative surface (`tools/*.json`,
//! `commands/*.md`, `skills/*/SKILL.md`), and refuses the JavaScript entry
//! with a named [`Refusal`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::bridge;
use super::declarative;
use super::{
    dedup_capabilities, manifest_error, manifest_string, read_json, read_prefix,
    resolve_package_path, Capability, ClientManifest, Contribution, Extension, ExtensionDetails,
    ExtensionError, ExtensionStandard, LoadedPackage, Refusal, RefusedFeature, Trust, UiPanel,
};

const STANDARD: ExtensionStandard = ExtensionStandard::DeepSeekHarness;
const ENTRY_SCAN_LIMIT: usize = 256 * 1024;

const HOST_MARKERS: &[(&str, Capability)] = &[
    ("ctx.tools.register", Capability::Tools),
    ("ctx.commands.register", Capability::Commands),
    ("ctx.skills.register", Capability::Skills),
    ("ctx.skills.registerProvider", Capability::Skills),
    ("ctx.on", Capability::Hooks),
    // The LLM adapter seam: what a model-provider package registers its
    // endpoint through. It is a host marker like the rest, so a package that
    // calls it reports the models capability whether or not it also ships the
    // declarative providers/*.json carrier.
    ("ctx.llm.registerAdapter", Capability::Models),
];

const CLIENT_MARKERS: &[(&str, Capability)] = &[
    ("ctx.slots.register", Capability::Panels),
    ("ctx.slots.inject", Capability::Panels),
];

/// Load one DeepSeek Harness package directory, deciding whether its host
/// entry may be EXECUTED.
///
/// This is the one load path both trust states share, and the difference is
/// exactly one fact: an untrusted package's entry is refused with a named
/// [`Refusal`] and NO process is started, while a trusted package's entry runs
/// in the bridge and every registration it makes becomes a real contribution in
/// the same registry the declarative files feed.
///
/// A trusted package whose entry throws, times out, or cannot be reached
/// yields ONE named [`Refusal`] and NOTHING registered: the load is
/// all-or-nothing, so a half-loaded plugin is impossible. Its declarative
/// files are still read, because they are facts about the package that do not
/// depend on the script.
pub fn load_with_trust(root: &Path, trust: Trust) -> Result<LoadedPackage, ExtensionError> {
    load_inner(root, trust)
}
/// Load one DeepSeek Harness package directory.
pub fn load(root: &Path) -> Result<LoadedPackage, ExtensionError> {
    load_inner(root, Trust::Untrusted)
}

fn load_inner(root: &Path, trust: Trust) -> Result<LoadedPackage, ExtensionError> {
    let manifest_path = root.join("package.json");
    if !manifest_path.is_file() {
        return Err(ExtensionError::WrongStandard {
            path: root.to_path_buf(),
            expected: STANDARD,
            found: ExtensionStandard::Codex,
        });
    }
    let value = read_json(&manifest_path, STANDARD)?;
    let object = value.as_object().ok_or_else(|| {
        manifest_error(
            STANDARD,
            &manifest_path,
            "package.json must be a JSON object",
        )
    })?;
    let dsh = object
        .get("dsh")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            manifest_error(
                STANDARD,
                &manifest_path,
                "package.json has no `dsh` object; this is not a DeepSeek Harness package",
            )
        })?;
    let id = manifest_string(object, "name", STANDARD, &manifest_path)?;
    let version = manifest_string(object, "version", STANDARD, &manifest_path)?;
    let package_description = object
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut capabilities = BTreeSet::new();
    let mut contributions = Vec::new();
    let mut refusals = Vec::new();
    let mut details = ExtensionDetails::default();
    // Set only when a TRUSTED host entry ran; the handle keeps the plugin's
    // process alive so the tools it registered stay callable.
    let mut host_session = None;

    if let Some(raw) = dsh.get("manifestVersion") {
        let version = raw.as_u64().ok_or_else(|| {
            manifest_error(
                STANDARD,
                &manifest_path,
                "dsh.manifestVersion must be a non-negative integer",
            )
        })?;
        if version != 1 {
            return Err(manifest_error(
                STANDARD,
                &manifest_path,
                format!("unsupported dsh.manifestVersion {version}; this shim reads format 1"),
            ));
        }
        details.manifest_version = Some(version as u32);
    }

    if let Some(engines) = object.get("engines").and_then(Value::as_object) {
        details.engines = engines
            .get("dsh")
            .and_then(Value::as_str)
            .map(str::to_string);
    }

    let mut bundle_refusals = Vec::new();
    if let Some(bundle) = dsh.get("bundle") {
        let bundle = bundle.as_object().ok_or_else(|| {
            manifest_error(STANDARD, &manifest_path, "dsh.bundle must be an object")
        })?;
        let patches = match bundle.get("patch") {
            Some(Value::String(path)) => vec![path.clone()],
            Some(Value::Array(items)) => items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    item.as_str().map(str::to_string).ok_or_else(|| {
                        manifest_error(
                            STANDARD,
                            &manifest_path,
                            format!("dsh.bundle.patch[{index}] must be a string"),
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => {
                return Err(manifest_error(
                    STANDARD,
                    &manifest_path,
                    "dsh.bundle.patch must be a string or an array of strings",
                ));
            }
            None => Vec::new(),
        };
        for raw in patches {
            let patch_path = resolve_package_path(root, &raw, STANDARD, &manifest_path)?;
            if !patch_path.is_file() {
                return Err(manifest_error(
                    STANDARD,
                    &manifest_path,
                    format!("dsh.bundle.patch '{}' does not exist", patch_path.display()),
                ));
            }
            details
                .bundle_patch
                .get_or_insert_with(|| patch_path.clone());
            for row in parse_bundle_patch(&patch_path)? {
                details.bundle_rows.push(row.id.clone());
                if row.mcp {
                    capabilities.insert(Capability::Tools);
                    refusals.push(Refusal::new(
                        &id,
                        RefusedFeature::McpServer,
                        Some(patch_path.clone()),
                        format!(
                            "the bundle patch mounts the MCP client at row '{}', which requires a server process this shim does not start",
                            row.id
                        ),
                    ));
                }
            }
            bundle_refusals.push(Refusal::new(
                &id,
                RefusedFeature::BundleMount,
                Some(patch_path.clone()),
                "the bundle patch was parsed for inventory, but mounting its Cordis rows requires the DeepSeek Harness loader, which this shim does not embed",
            ));
        }
    }

    let mut display_name = id.clone();
    let mut description = package_description;
    if let Some(meta) = read_locale_meta(root)? {
        if let Some(title) = meta.title {
            display_name = title;
        }
        if let Some(meta_description) = meta.description {
            description = Some(meta_description);
        }
    }

    if let Some(client) = dsh.get("client") {
        let client = client.as_object().ok_or_else(|| {
            manifest_error(STANDARD, &manifest_path, "dsh.client must be an object")
        })?;
        details.client = Some(parse_client_manifest(client, &manifest_path)?);
        capabilities.insert(Capability::Panels);
        contributions.push(Contribution::Panel(UiPanel::new(
            id.clone(),
            display_name.clone(),
        )));
    }

    details.icon = package_icon(root, object, &manifest_path)?;

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
    let skills = declarative::read_skills(&root.join("skills"), STANDARD)?;
    if !skills.is_empty() {
        capabilities.insert(Capability::Skills);
        contributions.extend(skills);
    }
    let agents = declarative::read_agents(root, STANDARD)?;
    if !agents.is_empty() {
        capabilities.insert(Capability::Prompts);
        contributions.extend(agents);
    }
    let providers = declarative::read_providers(root, STANDARD)?;
    if !providers.is_empty() {
        capabilities.insert(Capability::Models);
        contributions.extend(providers);
    }

    if let Some(entry) = host_entry(root, object, &manifest_path)? {
        details.host_entry = Some(entry.clone());
        if !entry.is_file() {
            return Err(manifest_error(
                STANDARD,
                &manifest_path,
                format!("the host entry '{}' does not exist", entry.display()),
            ));
        }
        let source = read_prefix(&entry, ENTRY_SCAN_LIMIT)?;
        let found = scan_markers(&source, HOST_MARKERS);
        for capability in &found {
            capabilities.insert(*capability);
        }
        if trust.permits_execution() {
            // A trusted package's entry runs in the bridge and every registration
            // it makes lands in this same contribution list. A failure is ONE
            // named refusal and contributes nothing: a half-loaded plugin is not
            // a state this loader can produce.
            match bridge::load_entry(root, &entry, STANDARD, serde_json::json!({})) {
                Ok(bridged) => {
                    for contribution in &bridged.contributions {
                        capabilities.insert(capability_for(contribution.kind()));
                    }
                    contributions.extend(bridged.contributions);
                    refusals.extend(bridged.refusals);
                    // The session must outlive the parse for the tools to be
                    // callable; a host that wants to call them keeps this handle.
                    host_session = Some(bridged.session);
                }
                Err(error) => {
                    refusals.push(bridge::refusal_for(&id, &entry, &error));
                }
            }
            // The bundled client module is browser JavaScript for the DSH web
            // renderer; this host has none, so it stays refused whether or not the
            // host entry ran.
        } else {
            refusals.push(Refusal::new(
                &id,
                RefusedFeature::ImperativeHostEntry,
                Some(entry),
                imperative_reason("host entry", &found),
            ));
        }
    }

    if details.client.is_some() {
        if let Some(entry) = export_path(root, object, "./client", &manifest_path)? {
            details.client_entry = Some(entry.clone());
            if !entry.is_file() {
                return Err(manifest_error(
                    STANDARD,
                    &manifest_path,
                    format!("the client entry '{}' does not exist", entry.display()),
                ));
            }
            let source = read_prefix(&entry, ENTRY_SCAN_LIMIT)?;
            let found = scan_markers(&source, CLIENT_MARKERS);
            for capability in &found {
                capabilities.insert(*capability);
            }
            refusals.push(Refusal::new(
                &id,
                RefusedFeature::ImperativeClientEntry,
                Some(entry),
                imperative_reason("client entry", &found),
            ));
        }
    }

    refusals.extend(bundle_refusals);

    let mut extension = Extension::new(id.clone(), display_name, version, STANDARD, root)
        .with_manifest(manifest_path)
        .with_capabilities(dedup_capabilities(capabilities))
        .with_details(details)
        .with_contributions(contributions);
    if let Some(description) = description {
        extension = extension.with_description(description);
    }
    let mut package = LoadedPackage::new(extension, refusals);
    package.session = host_session;
    Ok(package)
}

/// The capability a contribution delivers, for the declaration list.
///
/// A bridged registration and a declarative file both arrive as the same
/// [`Contribution`] variants, so ONE mapping serves both paths: the declared
/// capability set cannot drift from what loaded.
fn capability_for(kind: super::ContributionKind) -> Capability {
    match kind {
        super::ContributionKind::Tool => Capability::Tools,
        super::ContributionKind::Hook => Capability::Hooks,
        super::ContributionKind::Command => Capability::Commands,
        super::ContributionKind::Skill | super::ContributionKind::SkillDir => Capability::Skills,
        super::ContributionKind::Panel => Capability::Panels,
        super::ContributionKind::Prompt => Capability::Prompts,
        super::ContributionKind::Provider => Capability::Models,
    }
}
struct PatchRow {
    id: String,
    mcp: bool,
}

fn parse_bundle_patch(path: &Path) -> Result<Vec<PatchRow>, ExtensionError> {
    let text = super::read_text(path)?;
    let value: serde_yaml::Value = serde_yaml::from_str(&text).map_err(|error| {
        manifest_error(
            STANDARD,
            path,
            format!("bundle patch is not valid YAML: {error}"),
        )
    })?;
    let sequence = value
        .as_sequence()
        .ok_or_else(|| manifest_error(STANDARD, path, "bundle patch must be a YAML sequence"))?;
    let mut rows = Vec::new();
    for item in sequence {
        let Some(insert) = yaml_get(item, "insert") else {
            continue;
        };
        let Some(items) = insert.as_sequence() else {
            return Err(manifest_error(
                STANDARD,
                path,
                "bundle patch `insert` must be a sequence",
            ));
        };
        for row in items {
            let id = yaml_get(row, "id")
                .and_then(serde_yaml::Value::as_str)
                .ok_or_else(|| {
                    manifest_error(
                        STANDARD,
                        path,
                        "every inserted bundle row needs a string `id`",
                    )
                })?;
            let name = yaml_get(row, "name")
                .and_then(serde_yaml::Value::as_str)
                .ok_or_else(|| {
                    manifest_error(
                        STANDARD,
                        path,
                        "every inserted bundle row needs a string `name`",
                    )
                })?;
            rows.push(PatchRow {
                id: id.to_string(),
                mcp: name == "@deepseek-ai/dsh-mcp-client",
            });
        }
    }
    Ok(rows)
}

fn yaml_get<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    let key = serde_yaml::Value::String(key.to_string());
    value.as_mapping()?.get(&key)
}

struct LocaleMeta {
    title: Option<String>,
    description: Option<String>,
}

fn read_locale_meta(root: &Path) -> Result<Option<LocaleMeta>, ExtensionError> {
    let path = root.join("locale").join("en.json");
    if !path.is_file() {
        return Ok(None);
    }
    let value = read_json(&path, STANDARD)?;
    let meta = value.get("meta").and_then(Value::as_object);
    Ok(Some(LocaleMeta {
        title: meta
            .and_then(|meta| meta.get("title"))
            .and_then(Value::as_str)
            .map(str::to_string),
        description: meta
            .and_then(|meta| meta.get("description"))
            .and_then(Value::as_str)
            .map(str::to_string),
    }))
}

fn parse_client_manifest(
    client: &serde_json::Map<String, Value>,
    manifest: &Path,
) -> Result<ClientManifest, ExtensionError> {
    Ok(ClientManifest {
        platform: client
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or("web")
            .to_string(),
        immediately: client
            .get("immediately")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        inject: string_array(client, "inject", manifest)?,
        external: string_array(client, "external", manifest)?,
    })
}

fn string_array(
    object: &serde_json::Map<String, Value>,
    key: &str,
    manifest: &Path,
) -> Result<Vec<String>, ExtensionError> {
    match object.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                item.as_str().map(str::to_string).ok_or_else(|| {
                    manifest_error(
                        STANDARD,
                        manifest,
                        format!("dsh.client.{key}[{index}] must be a string"),
                    )
                })
            })
            .collect(),
        Some(_) => Err(manifest_error(
            STANDARD,
            manifest,
            format!("dsh.client.{key} must be an array of strings"),
        )),
    }
}

fn package_icon(
    root: &Path,
    object: &serde_json::Map<String, Value>,
    manifest: &Path,
) -> Result<Option<PathBuf>, ExtensionError> {
    let raw = object
        .get("icon")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| export_string(object, "./icon"));
    let Some(raw) = raw else {
        return Ok(None);
    };
    let path = resolve_package_path(root, &raw, STANDARD, manifest)?;
    Ok(path.is_file().then_some(path))
}

fn host_entry(
    root: &Path,
    object: &serde_json::Map<String, Value>,
    manifest: &Path,
) -> Result<Option<PathBuf>, ExtensionError> {
    let raw = export_string(object, ".").or_else(|| {
        object
            .get("main")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let Some(raw) = raw else {
        return Ok(None);
    };
    Ok(Some(resolve_package_path(root, &raw, STANDARD, manifest)?))
}

fn export_path(
    root: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
    manifest: &Path,
) -> Result<Option<PathBuf>, ExtensionError> {
    let Some(raw) = export_string(object, key) else {
        return Ok(None);
    };
    Ok(Some(resolve_package_path(root, &raw, STANDARD, manifest)?))
}

fn export_string(object: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    let exports = object.get("exports")?;
    if let Some(text) = exports.as_str() {
        return (key == ".").then(|| text.to_string());
    }
    let map = exports.as_object()?;
    export_target(map.get(key)?).map(str::to_string)
}

fn export_target(value: &Value) -> Option<&str> {
    if let Some(text) = value.as_str() {
        return Some(text);
    }
    let map = value.as_object()?;
    for key in ["default", "import", "require", "node"] {
        if let Some(found) = map.get(key).and_then(export_target) {
            return Some(found);
        }
    }
    None
}

fn scan_markers(source: &str, markers: &[(&str, Capability)]) -> Vec<Capability> {
    let mut found = Vec::new();
    for (needle, capability) in markers {
        if source.contains(needle) && !found.contains(capability) {
            found.push(*capability);
        }
    }
    found
}

fn imperative_reason(kind: &str, found: &[Capability]) -> String {
    let calls = if found.is_empty() {
        "the entry was not executed".to_string()
    } else {
        format!(
            "its source calls {}",
            found
                .iter()
                .map(|capability| capability.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    // The tail differs when the package ALSO ships a declarative carrier for
    // the capability its script would have registered. Saying "refused" over a
    // contribution that did load would be a false alarm the reader cannot act
    // on, so the sentence names what actually arrived.
    let landed = if found.contains(&Capability::Models) {
        " Declarative contributions in this package were loaded, and the model providers it declares in providers/*.json are live."
    } else {
        " Declarative contributions in this package were loaded."
    };
    format!(
        "the {kind} is DeepSeek Harness JavaScript (`apply(ctx, config)`); {calls}. The reference registers tools, commands, skills, hooks and LLM adapters by executing script (`ctx.tools.register`, `ctx.commands.register`, `ctx.skills.register`, `ctx.on`, `ctx.llm.registerAdapter`), and this build has no JavaScript runtime, so the entry was refused.{landed}"
    )
}
