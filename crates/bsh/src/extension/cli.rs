//! The developer CLI behind `bsh ext`.
//!
//! The CLI is the first consumer of the same host the desktop build uses, which
//! is the point: validate, install, list, grant and doctor must exercise the
//! real store, policy and lifecycle, not a second implementation that agrees by
//! luck. It opens no window and needs no GPUI.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::extension::capability::{Capability, GrantScope};
use crate::extension::host::{DiagnosticsReport, ExtensionHost, ExtensionStatus, HostConfig};
use crate::extension::manifest::MANIFEST_FILE;
use crate::extension::package::{package_hash, ExtensionStore, Package};
use crate::extension::resolver;
use crate::extension::runtime::declarative::{load_contributions, DeclarativeRuntime};
use crate::extension::runtime::mcp::McpRuntime;
use crate::extension::runtime::process::ProcessRuntime;
use crate::extension::signing::{
    generate_key, sign_package, verify_package, SignatureVerification, TrustStore,
};
use crate::extension::sources::{self, RegistryList};
use crate::extension::types::{ExtensionId, ExtensionSource, RuntimeKind, TrustLevel};

const USAGE: &str = "bsh ext <command> [options]

Commands:
  new <id> [--dir <path>] [--force]   scaffold a declarative extension package
  validate <path>                     parse and validate a package
  hash <path>                         print the canonical package hash
  build <path> [--out <dir>]          validate and build a .tar.gz release
  package <path> [--out <dir>]        alias for build
  test <path>                         validate the package and its contributions
  install <path|archive|git+url>      install a package into the managed store
              [--registry <name|url> <id>[@version]]
  uninstall <id> [--purge-data]       remove a package; keep data by default
  list                                list installed and built-in extensions
  info <id>                           show one extension in detail
  enable <id>                         enable an extension
  disable <id>                        disable an extension
  versions <id>                      list installed versions and the active one
  update <path>                      install a new version and activate it
  rollback <id>                      activate the previous installed version
  quarantine <id> [--reason <text>]  move code to quarantine and disable it
  grant <id> <capability> [options]   grant a capability (--scope, --note)
  revoke <id> <capability>            revoke a capability
  capabilities                        list the capability vocabulary
  logs <id>                           show the audit and runtime logs for an id
  config <id> [set <json|file>|schema]
                                      read or write an extension's configuration
  metrics                             print call, event and subscriber counters
  providers                           list model and agent provider descriptors
  doctor                              check the store, hashes, dependencies and policy
  keygen <publisher> [--out <path>]   create an Ed25519 publisher key pair
  sign <path> --publisher <name> --key <file-or-base64>
                                      write package.sig over id, version and hash
  verify <path>                       verify a package signature against the trust store
  trust <publisher> <public-key>      trust a publisher key
  untrust <publisher>                 remove a trusted publisher
  publishers                          list trusted publishers
  registry add <name> <url>           add an official or private registry
  registry list                       list configured registries
  registry remove <name>              remove a registry
  search [query] [--registry <name>]  search configured registries

Global options:
  --root <path>   override the managed extension root ($BSH_EXTENSION_ROOT)
  --json          machine-readable output where supported

Exit codes: 0 success, 1 failure, 2 usage.";

/// Run the CLI and return the process exit code.
pub(crate) fn run(args: &[String]) -> i32 {
    let mut rest = args.to_vec();
    let mut json = false;
    let mut root: Option<PathBuf> = None;
    while let Some(first) = rest.first().cloned() {
        if first == "--json" {
            json = true;
            rest.remove(0);
        } else if first == "--root" {
            rest.remove(0);
            let Some(value) = rest.first().cloned() else {
                eprintln!("--root needs a path");
                return 2;
            };
            root = Some(PathBuf::from(value));
            rest.remove(0);
        } else if let Some(value) = first.strip_prefix("--root=") {
            root = Some(PathBuf::from(value));
            rest.remove(0);
        } else {
            break;
        }
    }
    let Some(command) = rest.first().cloned() else {
        println!("{USAGE}");
        return 2;
    };
    let result = match command.as_str() {
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            return 0;
        }
        "new" => command_new(&rest[1..]),
        "validate" => command_validate(&rest[1..]),
        "hash" => command_hash(&rest[1..]),
        "build" | "package" => command_package(&rest[1..]),
        "test" => command_test(&rest[1..]),
        "install" => command_install(&rest[1..], root.clone()),
        "uninstall" => command_uninstall(&rest[1..], root.clone()),
        "list" => command_list(root.clone(), json),
        "info" => command_info(&rest[1..], root.clone(), json),
        "enable" | "disable" => command_toggle(&command, &rest[1..], root.clone()),
        "versions" => command_versions(&rest[1..], root.clone()),
        "update" => command_install(&rest[1..], root.clone()),
        "rollback" => command_rollback(&rest[1..], root.clone()),
        "quarantine" => command_quarantine(&rest[1..], root.clone()),
        "grant" => command_grant(&rest[1..], root.clone()),
        "revoke" => command_revoke(&rest[1..], root.clone()),
        "capabilities" => command_capabilities(),
        "logs" => command_logs(&rest[1..], root.clone()),
        "config" => command_config(&rest[1..], root.clone()),
        "doctor" => command_doctor(root.clone(), json),
        "metrics" => command_metrics(root.clone()),
        "providers" => command_providers(root.clone()),
        "keygen" => command_keygen(&rest[1..]),
        "sign" => command_sign(&rest[1..]),
        "verify" => command_verify(&rest[1..], root.clone()),
        "trust" => command_trust(&rest[1..], root.clone()),
        "untrust" => command_untrust(&rest[1..], root.clone()),
        "publishers" => command_publishers(root.clone()),
        "registry" => command_registry(&rest[1..], root.clone()),
        "search" => command_search(&rest[1..], root.clone()),
        other => {
            eprintln!("unknown command {other:?}");
            println!("{USAGE}");
            return 2;
        }
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn config(root: Option<PathBuf>) -> HostConfig {
    let root = root
        .or_else(|| {
            std::env::var("BSH_EXTENSION_ROOT")
                .ok()
                .map(|raw| PathBuf::from(shellexpand::tilde(&raw).into_owned()))
        })
        .unwrap_or_else(|| crate::settings::dir().join("extensions"));
    HostConfig {
        root,
        workspace: std::env::current_dir().ok(),
        disabled: BTreeSet::new(),
        preset_allowed: None,
    }
}

fn open_host(root: Option<PathBuf>) -> Result<ExtensionHost, Box<dyn std::error::Error>> {
    Ok(ExtensionHost::open(config(root))?)
}

fn one_argument(args: &[String], usage: &str) -> Result<String, String> {
    args.first()
        .cloned()
        .ok_or_else(|| format!("usage: {usage}"))
}

fn command_new(args: &[String]) -> Result<i32, String> {
    let id = one_argument(args, "bsh ext new <id> [--dir <path>] [--force]")?;
    let id = ExtensionId::parse(id).map_err(|error| error.to_string())?;
    let mut directory = PathBuf::from(id.as_str());
    let mut force = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--dir" => {
                directory = PathBuf::from(
                    args.get(index + 1)
                        .ok_or_else(|| "--dir needs a path".to_string())?,
                );
                index += 2;
            }
            "--force" => {
                force = true;
                index += 1;
            }
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    if directory.exists() && !force {
        return Err(format!(
            "{} already exists; pass --force to scaffold into it",
            directory.display()
        ));
    }
    std::fs::create_dir_all(directory.join("tools")).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(directory.join("commands")).map_err(|error| error.to_string())?;
    let name = title_case(id.namespace());
    let manifest = format!(
        r#"schema_version = "1.0"

[extension]
id = "{id}"
name = "{name}"
version = "0.1.0"
description = "A BSH extension"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"
startup = "on-demand"

[capabilities]
required = ["agent.tools.register"]
optional = ["workspace.read"]

[contributes]
tools = ["echo"]
commands = ["refresh"]

[resources]
max_execution_ms = 5000
"#
    );
    std::fs::write(directory.join(MANIFEST_FILE), manifest).map_err(|error| error.to_string())?;
    std::fs::write(
        directory.join("tools/echo.json"),
        r#"{
  "description": "Echo the arguments back to the model",
  "input_schema": {
    "type": "object",
    "properties": {}
  },
  "implementation": { "kind": "echo_arguments" }
}
"#,
    )
    .map_err(|error| error.to_string())?;
    std::fs::write(
        directory.join("commands/refresh.md"),
        "---\ndescription: Refresh the extension\n---\nRefresh the view.\n",
    )
    .map_err(|error| error.to_string())?;
    std::fs::write(
        directory.join("README.md"),
        format!(
            "# {name}\n\nA BSH extension. Run 'bsh ext validate {}' before installing it.\n",
            directory.display()
        ),
    )
    .map_err(|error| error.to_string())?;
    println!("created {} for {id}", directory.display());
    println!("next: bsh ext validate {}", directory.display());
    Ok(0)
}

fn title_case(raw: &str) -> String {
    raw.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => {
                    let mut word = first.to_uppercase().to_string();
                    word.push_str(characters.as_str());
                    word
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn command_validate(args: &[String]) -> Result<i32, String> {
    let path = PathBuf::from(one_argument(args, "bsh ext validate <path>")?);
    let package = Package::load(&path, ExtensionSource::Development, TrustLevel::Local)
        .map_err(|error| error.to_string())?;
    println!("id:      {}", package.manifest.extension.id);
    println!("version: {}", package.manifest.extension.version);
    println!("runtime: {}", package.manifest.runtime.kind);
    if package.hash.is_empty() {
        println!("hash:    (not computed while the manifest is invalid)");
    } else {
        println!("hash:    {}", package.hash);
    }
    for warning in &package.report.warnings {
        println!("warning: {warning}");
    }
    for error in &package.report.errors {
        println!("error:   {error}");
    }
    let mut valid = package.report.is_valid();
    if valid {
        if let Err(error) = validate_runtime(&package) {
            println!("error:   {error}");
            valid = false;
        }
    }
    if valid {
        println!("valid");
        Ok(0)
    } else {
        println!("invalid");
        Ok(1)
    }
}

/// Load the runtime's contribution files without activating anything.
///
/// Manifest validation alone cannot see a missing tools/<id>.json or an
/// unparseable panel. validate and install call this so a package is refused
/// before it is copied, not on the first host build after it was installed.
fn validate_runtime(package: &Package) -> Result<(), String> {
    match package.manifest.runtime.kind {
        RuntimeKind::Declarative => DeclarativeRuntime::new(&package.root, &package.manifest)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        RuntimeKind::Process => {
            ProcessRuntime::new(&package.manifest, &package.root)
                .map_err(|error| error.to_string())?;
            load_contributions(&package.root, &package.manifest)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        RuntimeKind::Mcp => McpRuntime::new(&package.manifest, &package.root, "{}".to_string())
            .map(|_| ())
            .map_err(|error| error.to_string()),
        _ => Ok(()),
    }
}

fn command_hash(args: &[String]) -> Result<i32, String> {
    let path = PathBuf::from(one_argument(args, "bsh ext hash <path>")?);
    match package_hash(&path) {
        Ok(hash) => {
            println!("{hash}");
            Ok(0)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn command_install(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let mut spec = one_argument(args, "bsh ext install <path|archive|git+url>")?;
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let mut registry: Option<String> = None;
    let mut requested_version: Option<String> = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--registry" => {
                registry = Some(
                    args.get(index + 1)
                        .ok_or_else(|| "--registry needs a name or URL".to_string())?
                        .clone(),
                );
                spec = args
                    .get(index + 2)
                    .cloned()
                    .ok_or_else(|| "--registry needs a package id".to_string())?;
                index += 3;
            }
            "--version" => {
                requested_version = Some(
                    args.get(index + 1)
                        .ok_or_else(|| "--version needs a value".to_string())?
                        .clone(),
                );
                index += 2;
            }
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    let fetched = match registry {
        Some(registry) => {
            let url = if registry.starts_with("http://") || registry.starts_with("https://") {
                registry
            } else {
                RegistryList::load(&store.state_root())
                    .map_err(|error| error.to_string())?
                    .registries
                    .get(&registry)
                    .map(|entry| entry.url.clone())
                    .ok_or_else(|| format!("unknown registry {registry:?}"))?
            };
            let (id, version) = match spec.split_once('@') {
                Some((id, version)) => (id.to_string(), Some(version.to_string())),
                None => (spec, requested_version),
            };
            sources::fetch_index_package(&url, &id, version.as_deref(), &store.cache_root())
                .map_err(|error| error.to_string())?
        }
        None => sources::resolve(&spec, &store.cache_root()).map_err(|error| error.to_string())?,
    };
    let candidate = Package::load(&fetched.root, fetched.origin.clone(), TrustLevel::Local)
        .map_err(|error| error.to_string())?;
    if !candidate.report.is_valid() {
        return Err(format!(
            "manifest is invalid: {}",
            candidate.error_message()
        ));
    }
    validate_runtime(&candidate)?;
    let (installed, _) = store.scan_installed().map_err(|error| error.to_string())?;
    let mut available = installed;
    available.push(candidate.clone());
    let roots = vec![candidate.id().clone()];
    if let Err(error) = resolver::resolve(&available, &roots) {
        return Err(format!("dependencies do not resolve: {error}"));
    }
    let package = sources::install_fetched(&store, &fetched).map_err(|error| error.to_string())?;
    println!("installed {} {}", package.id(), package.version());
    println!("source:  {}", fetched.origin.label());
    println!("hash:    {}", package.hash);
    println!("path:    {}", package.root.display());
    Ok(0)
}

fn command_uninstall(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let id = ExtensionId::parse(one_argument(args, "bsh ext uninstall <id> [--purge-data]")?)
        .map_err(|error| error.to_string())?;
    let purge = args.iter().any(|arg| arg == "--purge-data");
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    store
        .uninstall(&id, purge)
        .map_err(|error| error.to_string())?;
    println!(
        "uninstalled {id}{}",
        if purge { " and purged its data" } else { "" }
    );
    Ok(0)
}

fn command_list(root: Option<PathBuf>, json: bool) -> Result<i32, String> {
    let host = open_host(root).map_err(|error| error.to_string())?;
    let statuses = host.statuses();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&statuses).unwrap_or_default()
        );
        return Ok(0);
    }
    println!("root: {}", host.root().display());
    println!(
        "{:<34} {:<10} {:<12} {:<10} {:<8} TOOLS",
        "ID", "VERSION", "STATE", "TRUST", "APPLIED"
    );
    for status in statuses {
        println!(
            "{:<34} {:<10} {:<12} {:<10} {:<8} {}",
            status.id,
            status.version,
            status.state.as_str(),
            status.trust,
            if status.applied { "yes" } else { "no" },
            status.tools.join(", ")
        );
    }
    Ok(0)
}

fn command_info(args: &[String], root: Option<PathBuf>, json: bool) -> Result<i32, String> {
    let id = one_argument(args, "bsh ext info <id>")?;
    let host = open_host(root).map_err(|error| error.to_string())?;
    let Some(status) = host.statuses().into_iter().find(|status| status.id == id) else {
        return Err(format!("extension {id} is not installed"));
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status).unwrap_or_default()
        );
        return Ok(0);
    }
    print_status(&status);
    Ok(0)
}

fn print_status(status: &ExtensionStatus) {
    println!("id:          {}", status.id);
    println!("name:        {}", status.name);
    println!("version:     {}", status.version);
    println!("runtime:     {}", status.runtime);
    println!("trust:       {}", status.trust);
    println!("source:      {}", status.source);
    println!("root:        {}", status.root);
    println!(
        "hash:        {}",
        if status.hash.is_empty() {
            "(built-in)"
        } else {
            &status.hash
        }
    );
    println!("signature:   {}", status.signature);
    if let Some(publisher) = &status.publisher {
        println!("publisher:   {publisher}");
    }
    println!("state:       {}", status.state);
    println!("health:      {}", status.health.as_str());
    println!("configured:  {}", status.configured);
    println!("applied:     {}", status.applied);
    if !status.tools.is_empty() {
        println!("tools:       {}", status.tools.join(", "));
    }
    if !status.commands.is_empty() {
        println!("commands:    {}", status.commands.join(", "));
    }
    if !status.panels.is_empty() {
        println!("panels:      {}", status.panels.join(", "));
    }
    if !status.dependencies.is_empty() {
        println!("depends on:  {}", status.dependencies.join(", "));
    }
    if let Some(error) = &status.error {
        println!("error:       {error}");
    }
    for capability in &status.capabilities {
        let decision = if capability.allowed {
            capability
                .scope
                .as_deref()
                .map(|scope| format!("allowed ({scope})"))
                .unwrap_or_else(|| "allowed".to_string())
        } else {
            format!(
                "denied ({})",
                capability.reason.as_deref().unwrap_or("no reason")
            )
        };
        println!(
            "capability:  {} [{}] {decision}",
            capability.capability, capability.risk
        );
    }
}

fn command_toggle(command: &str, args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let id = ExtensionId::parse(one_argument(args, "bsh ext enable|disable <id>")?)
        .map_err(|error| error.to_string())?;
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let enabled = command == "enable";
    store
        .set_enabled(&id, enabled)
        .map_err(|error| error.to_string())?;
    println!("{command}d {id}");
    Ok(0)
}

fn command_grant(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let id = ExtensionId::parse(args.first().ok_or_else(|| {
        "usage: bsh ext grant <id> <capability> [--scope s] [--note n]".to_string()
    })?)
    .map_err(|error| error.to_string())?;
    let capability: Capability = args
        .get(1)
        .ok_or_else(|| "usage: bsh ext grant <id> <capability> [--scope s] [--note n]".to_string())?
        .parse()
        .map_err(|error: crate::extension::types::ExtensionError| error.to_string())?;
    let mut scope = default_scope(capability);
    let mut note = None;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--scope" => {
                let raw = args
                    .get(index + 1)
                    .ok_or_else(|| "--scope needs a value".to_string())?;
                scope = raw
                    .parse::<GrantScope>()
                    .map_err(|error| error.to_string())?;
                index += 2;
            }
            "--note" => {
                note = Some(
                    args.get(index + 1)
                        .ok_or_else(|| "--note needs a value".to_string())?
                        .clone(),
                );
                index += 2;
            }
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    let host = open_host(root).map_err(|error| error.to_string())?;
    host.policy()
        .grant(id.as_str(), capability, scope.clone(), note)
        .map_err(|error| error.to_string())?;
    println!("granted {capability} to {id} ({scope})");
    Ok(0)
}

fn default_scope(capability: Capability) -> GrantScope {
    match capability {
        Capability::WorkspaceRead | Capability::WorkspaceWrite => GrantScope::Workspace,
        _ => GrantScope::Any,
    }
}

fn command_revoke(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let id = ExtensionId::parse(
        args.first()
            .ok_or_else(|| "usage: bsh ext revoke <id> <capability>".to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let capability: Capability = args
        .get(1)
        .ok_or_else(|| "usage: bsh ext revoke <id> <capability>".to_string())?
        .parse()
        .map_err(|error: crate::extension::types::ExtensionError| error.to_string())?;
    let host = open_host(root).map_err(|error| error.to_string())?;
    let removed = host
        .policy()
        .revoke(id.as_str(), capability)
        .map_err(|error| error.to_string())?;
    println!(
        "{} {capability} from {id}",
        if removed { "revoked" } else { "no grant for" }
    );
    Ok(0)
}

fn command_capabilities() -> Result<i32, String> {
    println!("{:<26} {:<10} DESCRIPTION", "CAPABILITY", "RISK");
    for capability in Capability::ALL {
        println!(
            "{:<26} {:<10} {}",
            capability.as_str(),
            capability.risk().as_str(),
            capability.description()
        );
    }
    Ok(0)
}

fn command_logs(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let id = ExtensionId::parse(one_argument(args, "bsh ext logs <id>")?)
        .map_err(|error| error.to_string())?;
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let audit = store.audit_path();
    if audit.is_file() {
        let text = std::fs::read_to_string(&audit).map_err(|error| error.to_string())?;
        let needle = format!("\"extension\":\"{id}\"");
        let mut printed = 0usize;
        for line in text.lines().rev() {
            if line.contains(&needle) {
                println!("{line}");
                printed += 1;
                if printed >= 200 {
                    break;
                }
            }
        }
        if printed == 0 {
            println!("no audit entries for {id}");
        }
    } else {
        println!("no audit log yet at {}", audit.display());
    }
    let directory = store.log_dir(&id);
    if directory.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&directory) {
            let mut files: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_file())
                .collect();
            files.sort();
            for file in files {
                if let Ok(text) = std::fs::read_to_string(&file) {
                    println!("--- {} ---", file.display());
                    let lines: Vec<&str> = text.lines().collect();
                    for line in lines
                        .iter()
                        .rev()
                        .take(200)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        println!("{line}");
                    }
                }
            }
        }
    }
    Ok(0)
}

fn command_doctor(root: Option<PathBuf>, json: bool) -> Result<i32, String> {
    let host = open_host(root).map_err(|error| error.to_string())?;
    let report: DiagnosticsReport = host.doctor();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
        return Ok(
            if report.issues.iter().any(|issue| issue.severity == "error") {
                1
            } else {
                0
            },
        );
    }
    println!(
        "host:      {} (extension API {})",
        report.host_version, report.extension_api
    );
    println!("root:      {}", report.root);
    println!("extensions: {}", report.extensions.len());
    for issue in &report.issues {
        match &issue.extension {
            Some(extension) => println!("{:<8} {extension}: {}", issue.severity, issue.message),
            None => println!("{:<8} {}", issue.severity, issue.message),
        }
    }
    let errors = report
        .issues
        .iter()
        .filter(|issue| issue.severity == "error")
        .count();
    let warnings = report
        .issues
        .iter()
        .filter(|issue| issue.severity == "warning")
        .count();
    println!("{errors} error(s), {warnings} warning(s)");
    Ok(if errors > 0 { 1 } else { 0 })
}

fn command_keygen(args: &[String]) -> Result<i32, String> {
    let publisher = one_argument(args, "bsh ext keygen <publisher> [--out <path>]")?;
    let mut output = PathBuf::from(format!("{publisher}.key"));
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--out" => {
                output = PathBuf::from(
                    args.get(index + 1)
                        .ok_or_else(|| "--out needs a path".to_string())?,
                );
                index += 2;
            }
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    let (secret, public) = generate_key();
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
    }
    std::fs::write(&output, format!("{secret}\n")).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o600));
    }
    println!("publisher: {publisher}");
    println!("secret:    {} (keep it private)", output.display());
    println!("public:    {public}");
    println!("next:      bsh ext trust {publisher} {public}");
    Ok(0)
}

fn command_sign(args: &[String]) -> Result<i32, String> {
    let path = PathBuf::from(one_argument(
        args,
        "bsh ext sign <path> --publisher <name> --key <file-or-base64>",
    )?);
    let mut publisher = None;
    let mut key = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--publisher" => {
                publisher = Some(
                    args.get(index + 1)
                        .ok_or_else(|| "--publisher needs a name".to_string())?
                        .clone(),
                );
                index += 2;
            }
            "--key" => {
                key = Some(
                    args.get(index + 1)
                        .ok_or_else(|| "--key needs a file or base64 seed".to_string())?
                        .clone(),
                );
                index += 2;
            }
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    let publisher = publisher.ok_or_else(|| "--publisher is required".to_string())?;
    let key = key.ok_or_else(|| "--key is required".to_string())?;
    let key = if std::path::Path::new(&key).is_file() {
        std::fs::read_to_string(&key)
            .map_err(|error| error.to_string())?
            .trim()
            .to_string()
    } else {
        key
    };
    let signature = sign_package(&path, &publisher, &key).map_err(|error| error.to_string())?;
    println!("signed {publisher}");
    println!("algorithm: {}", signature.algorithm);
    println!("public:    {}", signature.public_key);
    Ok(0)
}

fn command_verify(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let path = PathBuf::from(one_argument(args, "bsh ext verify <path>")?);
    let manifest =
        crate::extension::manifest::Manifest::load(&path).map_err(|error| error.to_string())?;
    let hash = package_hash(&path).map_err(|error| error.to_string())?;
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let trust = TrustStore::load(store.trust_path()).map_err(|error| error.to_string())?;
    let verification = verify_package(
        &path,
        manifest.id().as_str(),
        &manifest.version().to_string(),
        &hash,
        &trust,
    );
    println!("signature: {}", verification.label());
    match &verification {
        SignatureVerification::Verified { publisher } => println!("publisher: {publisher}"),
        SignatureVerification::Untrusted { publisher, reason } => {
            println!("publisher: {publisher}");
            println!("reason:    {reason}");
        }
        SignatureVerification::Invalid { reason } => println!("reason:    {reason}"),
        SignatureVerification::Missing => {}
    }
    Ok(
        if matches!(verification, SignatureVerification::Invalid { .. }) {
            1
        } else {
            0
        },
    )
}

fn command_trust(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let publisher = one_argument(args, "bsh ext trust <publisher> <public-key>")?;
    let public_key = args
        .get(1)
        .ok_or_else(|| "usage: bsh ext trust <publisher> <public-key>".to_string())?
        .clone();
    let note = args
        .windows(2)
        .find(|pair| pair[0] == "--note")
        .map(|pair| pair[1].clone());
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let mut trust = TrustStore::load(store.trust_path()).map_err(|error| error.to_string())?;
    trust
        .trust(&publisher, &public_key, note)
        .map_err(|error| error.to_string())?;
    println!("trusted {publisher}");
    Ok(0)
}

fn command_untrust(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let publisher = one_argument(args, "bsh ext untrust <publisher>")?;
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let mut trust = TrustStore::load(store.trust_path()).map_err(|error| error.to_string())?;
    let removed = trust
        .revoke(&publisher)
        .map_err(|error| error.to_string())?;
    println!(
        "{} {publisher}",
        if removed { "untrusted" } else { "no trust for" }
    );
    Ok(0)
}

fn command_publishers(root: Option<PathBuf>) -> Result<i32, String> {
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let trust = TrustStore::load(store.trust_path()).map_err(|error| error.to_string())?;
    let publishers = trust.publishers();
    if publishers.is_empty() {
        println!("no trusted publishers");
        return Ok(0);
    }
    for (name, publisher) in publishers {
        println!("{name}\t{}", publisher.public_key);
    }
    Ok(0)
}

fn command_versions(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let id = ExtensionId::parse(one_argument(args, "bsh ext versions <id>")?)
        .map_err(|error| error.to_string())?;
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let versions = store.versions(&id).map_err(|error| error.to_string())?;
    let active = store.active_version(&id);
    for version in versions {
        println!(
            "{version}{}",
            if active.as_deref() == Some(version.as_str()) {
                " (active)"
            } else {
                ""
            }
        );
    }
    Ok(0)
}

fn command_rollback(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let id = ExtensionId::parse(one_argument(args, "bsh ext rollback <id>")?)
        .map_err(|error| error.to_string())?;
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let versions = store.versions(&id).map_err(|error| error.to_string())?;
    let active = store.active_version(&id);
    let target = match active
        .as_deref()
        .and_then(|active| versions.iter().position(|version| version == active))
    {
        Some(0) => None,
        Some(position) => Some(versions[position - 1].clone()),
        None => versions.last().cloned(),
    };
    let Some(target) = target else {
        return Err("no earlier version to roll back to".to_string());
    };
    store
        .set_active(&id, &target)
        .map_err(|error| error.to_string())?;
    println!("activated {id} {target}");
    Ok(0)
}

fn command_quarantine(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let id = ExtensionId::parse(one_argument(args, "bsh ext quarantine <id>")?)
        .map_err(|error| error.to_string())?;
    let reason = args
        .windows(2)
        .find(|pair| pair[0] == "--reason")
        .map(|pair| pair[1].clone())
        .unwrap_or_else(|| "reader requested".to_string());
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let destination = store
        .quarantine(&id, &reason)
        .map_err(|error| error.to_string())?;
    println!("quarantined {id} at {}", destination.display());
    Ok(0)
}

fn command_registry(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let action = args
        .first()
        .cloned()
        .ok_or_else(|| "usage: bsh ext registry add|list|remove ...".to_string())?;
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let mut registries =
        RegistryList::load(&store.state_root()).map_err(|error| error.to_string())?;
    match action.as_str() {
        "add" => {
            let name = args
                .get(1)
                .ok_or_else(|| "usage: bsh ext registry add <name> <url>".to_string())?;
            let url = args
                .get(2)
                .ok_or_else(|| "usage: bsh ext registry add <name> <url>".to_string())?;
            registries.add(name, url);
            registries
                .save(&store.state_root())
                .map_err(|error| error.to_string())?;
            println!("added registry {name} -> {url}");
        }
        "list" => {
            if registries.registries.is_empty() {
                println!("no registries");
            }
            for (name, registry) in registries.registries {
                println!("{name}\t{}", registry.url);
            }
        }
        "remove" => {
            let name = args
                .get(1)
                .ok_or_else(|| "usage: bsh ext registry remove <name>".to_string())?;
            registries.registries.remove(name);
            registries
                .save(&store.state_root())
                .map_err(|error| error.to_string())?;
            println!("removed registry {name}");
        }
        other => return Err(format!("unknown registry action {other:?}")),
    }
    Ok(0)
}

fn command_search(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let query = args
        .first()
        .filter(|value| !value.starts_with("--"))
        .cloned()
        .unwrap_or_default();
    let only = args
        .windows(2)
        .find(|pair| pair[0] == "--registry")
        .map(|pair| pair[1].clone());
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let registries = RegistryList::load(&store.state_root()).map_err(|error| error.to_string())?;
    let mut found = 0usize;
    for (name, registry) in registries.registries {
        if only.as_deref().is_some_and(|only| only != name) {
            continue;
        }
        let index = sources::fetch_index(&registry.url).map_err(|error| error.to_string())?;
        for package in index.packages {
            if !query.is_empty()
                && !package.id.contains(&query)
                && !package
                    .name
                    .as_deref()
                    .is_some_and(|name| name.contains(&query))
            {
                continue;
            }
            println!(
                "{}\t{}\t{}\t{}",
                package.id,
                package.version,
                package.name.unwrap_or_default(),
                package.description.unwrap_or_default()
            );
            found += 1;
        }
    }
    if found == 0 {
        println!("no matching packages");
    }
    Ok(0)
}

fn command_package(args: &[String]) -> Result<i32, String> {
    let path = PathBuf::from(one_argument(args, "bsh ext build <path> [--out <dir>]")?);
    let mut out = path.join("dist");
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--out" => {
                out = PathBuf::from(
                    args.get(index + 1)
                        .ok_or_else(|| "--out needs a directory".to_string())?,
                );
                index += 2;
            }
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    let package = Package::load(&path, ExtensionSource::Development, TrustLevel::Local)
        .map_err(|error| error.to_string())?;
    if !package.report.is_valid() {
        return Err(format!("manifest is invalid: {}", package.error_message()));
    }
    validate_runtime(&package)?;
    let hash = package_hash(&path).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&out).map_err(|error| error.to_string())?;
    let archive_path = out.join(format!("{}-{}.tar.gz", package.id(), package.version()));
    let temporary = archive_path.with_extension("tar.gz.tmp");
    let file = std::fs::File::create(&temporary).map_err(|error| error.to_string())?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::best());
    let mut builder = tar::Builder::new(encoder);
    let prefix = format!("{}-{}", package.id(), package.version());
    append_tree(&mut builder, &path, Path::new(&prefix)).map_err(|error| error.to_string())?;
    let encoder = builder.into_inner().map_err(|error| error.to_string())?;
    encoder.finish().map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &archive_path).map_err(|error| error.to_string())?;
    println!("built {}", archive_path.display());
    println!("hash:  {hash}");
    Ok(0)
}

fn append_tree(
    builder: &mut tar::Builder<flate2::write::GzEncoder<std::fs::File>>,
    source: &Path,
    archive: &Path,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            ".git" | "target" | "node_modules" | ".jj" | "dist"
        ) {
            continue;
        }
        let path = entry.path();
        let relative = archive.join(name.as_ref());
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            builder.append_dir(&relative, &path)?;
            append_tree(builder, &path, &relative)?;
        } else if metadata.is_file() {
            builder.append_path_with_name(&path, &relative)?;
        }
    }
    Ok(())
}

fn command_test(args: &[String]) -> Result<i32, String> {
    let path = PathBuf::from(one_argument(args, "bsh ext test <path>")?);
    let package = Package::load(&path, ExtensionSource::Development, TrustLevel::Local)
        .map_err(|error| error.to_string())?;
    if !package.report.is_valid() {
        println!("invalid: {}", package.error_message());
        return Ok(1);
    }
    validate_runtime(&package)?;
    println!("valid {}", package.id());
    println!("version: {}", package.version());
    println!("runtime: {}", package.manifest.runtime.kind);
    println!("tools: {}", package.manifest.contributes.tools.len());
    println!("commands: {}", package.manifest.contributes.commands.len());
    println!("panels: {}", package.manifest.contributes.panels.len());
    println!(
        "workflows: {}",
        package.manifest.contributes.workflows.len()
    );
    Ok(0)
}

fn command_config(args: &[String], root: Option<PathBuf>) -> Result<i32, String> {
    let id = ExtensionId::parse(one_argument(
        args,
        "bsh ext config <id> [set <json|file>|schema]",
    )?)
    .map_err(|error| error.to_string())?;
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let version = store
        .active_version(&id)
        .or_else(|| {
            store
                .versions(&id)
                .ok()
                .and_then(|versions| versions.last().cloned())
        })
        .ok_or_else(|| format!("{id} is not installed"))?;
    let package_root = store.installed_root().join(id.as_str()).join(&version);
    let manifest = crate::extension::manifest::Manifest::load(&package_root)
        .map_err(|error| error.to_string())?;
    let data_root = store.data_dir(&id);
    let action = args.get(1).map(String::as_str);
    match action {
        Some("schema") => {
            let Some(schema) = manifest
                .configuration
                .as_ref()
                .and_then(|configuration| configuration.schema.as_deref())
            else {
                return Err("this extension declares no configuration schema".to_string());
            };
            let text = std::fs::read_to_string(package_root.join(schema))
                .map_err(|error| error.to_string())?;
            println!("{text}");
        }
        Some("set") => {
            let raw = args
                .get(2)
                .ok_or_else(|| "usage: bsh ext config <id> set <json|file>".to_string())?;
            let text = if std::path::Path::new(raw).is_file() {
                std::fs::read_to_string(raw).map_err(|error| error.to_string())?
            } else {
                raw.clone()
            };
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(|error| error.to_string())?;
            let saved = crate::extension::config::save(&manifest, &package_root, &data_root, value)
                .map_err(|error| error.to_string())?;
            crate::extension::publish(
                crate::extension::topics::CONFIG_CHANGED,
                serde_json::json!({"extension": id.as_str()}),
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&saved.value).map_err(|error| error.to_string())?
            );
        }
        Some(other) => return Err(format!("unknown config action {other:?}")),
        None => {
            let loaded = crate::extension::config::load(&manifest, &package_root, &data_root)
                .map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&loaded.value).map_err(|error| error.to_string())?
            );
        }
    }
    Ok(0)
}

fn command_metrics(root: Option<PathBuf>) -> Result<i32, String> {
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let host = ExtensionHost::open(HostConfig {
        root: store.root().to_path_buf(),
        workspace: None,
        disabled: BTreeSet::new(),
        preset_allowed: None,
    })
    .map_err(|error| error.to_string())?;
    let metrics = host.metrics();
    println!(
        "{}",
        serde_json::to_string_pretty(&metrics).map_err(|error| error.to_string())?
    );
    Ok(0)
}

fn command_providers(root: Option<PathBuf>) -> Result<i32, String> {
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let host = ExtensionHost::open(HostConfig {
        root: store.root().to_path_buf(),
        workspace: None,
        disabled: BTreeSet::new(),
        preset_allowed: None,
    })
    .map_err(|error| error.to_string())?;
    let providers = host.provider_descriptors();
    println!(
        "{}",
        serde_json::to_string_pretty(&providers).map_err(|error| error.to_string())?
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::command_package;

    #[test]
    fn a_package_builds_a_readable_tarball() {
        let root = std::env::temp_dir().join(format!("bsh-cli-package-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("tools")).expect("tools");
        std::fs::write(
            root.join("extension.toml"),
            r#"
schema_version = "1.0"

[extension]
id = "com.example.built"
name = "Built"
version = "1.0.0"
description = "A built test package"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[contributes]
tools = ["echo"]
"#,
        )
        .expect("manifest");
        std::fs::write(
            root.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"},"implementation":{"kind":"echo_arguments"}}"#,
        )
        .expect("tool");
        let out = root.join("dist");
        command_package(&[
            root.display().to_string(),
            "--out".to_string(),
            out.display().to_string(),
        ])
        .expect("package");
        let archive = out.join("com.example.built-1.0.0.tar.gz");
        assert!(archive.is_file(), "{archive:?}");
        let file = std::fs::File::open(&archive).expect("archive");
        let decoder = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(decoder);
        let names: Vec<String> = tar
            .entries()
            .expect("entries")
            .filter_map(Result::ok)
            .filter_map(|entry| entry.path().ok().map(|path| path.display().to_string()))
            .collect();
        assert!(
            names.iter().any(|name| name.ends_with("extension.toml")),
            "{names:?}"
        );
        assert!(
            names.iter().any(|name| name.ends_with("tools/echo.json")),
            "{names:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
