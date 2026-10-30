//! The developer CLI behind `bsh ext`.
//!
//! The CLI is the first consumer of the same host the desktop build uses, which
//! is the point: validate, install, list, grant and doctor must exercise the
//! real store, policy and lifecycle, not a second implementation that agrees by
//! luck. It opens no window and needs no GPUI.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::extension::capability::{Capability, GrantScope};
use crate::extension::host::{DiagnosticsReport, ExtensionHost, ExtensionStatus, HostConfig};
use crate::extension::manifest::MANIFEST_FILE;
use crate::extension::package::{package_hash, ExtensionStore, Package};
use crate::extension::resolver;
use crate::extension::runtime::declarative::{load_contributions, DeclarativeRuntime};
use crate::extension::runtime::process::ProcessRuntime;
use crate::extension::types::{ExtensionId, ExtensionSource, RuntimeKind, TrustLevel};

const USAGE: &str = "bsh ext <command> [options]

Commands:
  new <id> [--dir <path>] [--force]   scaffold a declarative extension package
  validate <path>                     parse and validate a package
  hash <path>                         print the canonical package hash
  install <path>                      install a package into the managed store
  uninstall <id> [--purge-data]       remove a package; keep data by default
  list                                list installed and built-in extensions
  info <id>                           show one extension in detail
  enable <id>                         enable an extension
  disable <id>                        disable an extension
  grant <id> <capability> [options]   grant a capability (--scope, --note)
  revoke <id> <capability>            revoke a capability
  capabilities                        list the capability vocabulary
  logs <id>                           show the audit and runtime logs for an id
  doctor                              check the store, hashes, dependencies and policy

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
        "install" => command_install(&rest[1..], root.clone()),
        "uninstall" => command_uninstall(&rest[1..], root.clone()),
        "list" => command_list(root.clone(), json),
        "info" => command_info(&rest[1..], root.clone(), json),
        "enable" | "disable" => command_toggle(&command, &rest[1..], root.clone()),
        "grant" => command_grant(&rest[1..], root.clone()),
        "revoke" => command_revoke(&rest[1..], root.clone()),
        "capabilities" => command_capabilities(),
        "logs" => command_logs(&rest[1..], root.clone()),
        "doctor" => command_doctor(root.clone(), json),
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
    let path = PathBuf::from(one_argument(args, "bsh ext install <path>")?);
    let store = ExtensionStore::open(config(root).root).map_err(|error| error.to_string())?;
    let candidate = Package::load(&path, ExtensionSource::Development, TrustLevel::Local)
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
    let package = store.install(&path).map_err(|error| error.to_string())?;
    println!("installed {} {}", package.id(), package.version());
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
