//! Command-line surface of the two runtime subcommands.
//!
//! `bsh host` is the detached authority: it owns execution until it is asked to
//! stop, whether or not any client is watching. `bsh tui` is a client, and it
//! starts a Host of its own only for a data directory that has none - see
//! [`crate::runtime::embed::Authority`]. Both share the argument vocabulary
//! (`--data-dir`, `--workspace`, `--echo`), so either process opens the same
//! durable state whichever one happened to start first.
//!
//! A Host these subcommands start is configured the way `bsh` itself is: the
//! model, endpoint, credential, persona and workspace come from `settings.toml`
//! and the vault, so a turn typed in the terminal is the same request a turn
//! typed in the window would send. `[agent]` in `host.toml` remains the fallback
//! for a Host that runs with no shell configuration at all.

use std::path::{Path, PathBuf};

use crate::runtime::config::RuntimeConfig;
use crate::runtime::contract::ShutdownMode;
use crate::runtime::embed::Authority;
use crate::runtime::executor::ReactSettings;
use crate::runtime::host::RuntimeHost;
use crate::runtime::ids::SessionId;
use crate::runtime::server::IpcServer;
use crate::settings::Settings;
use crate::settings::workspace::WorkspaceSettings;

const HOST_USAGE: &str = "\
usage: bsh host [options]

Run the standalone BSH Runtime Host: it owns sessions, executions, approvals
and durable state, and serves them to clients over local IPC. It runs the model
bsh is configured with - `settings.toml`, the vault and the open workspace -
unless `[agent]` in `host.toml` is the only configuration it can read.

options:
  --data-dir <dir>   durable state directory (default <bsh data dir>/runtime)
  --workspace <dir>  directory the agent works in (default the workspace bsh
                     has open)
  --socket <path>    IPC socket path (default <data-dir>/host.sock)
  --echo             use the deterministic echo executor instead of a model
  --help             show this message
";

const TUI_USAGE: &str = "\
usage: bsh tui [options]

Run the terminal interface. When a Runtime Host already serves the data
directory - `bsh host`, or another `bsh tui` - the interface attaches to it and
leaves it running. Otherwise it starts a Host for this process, serves it on the
same local IPC endpoint and stops it again on quit, so `bsh tui` needs no second
command to be a complete runtime.

The interface shows sessions, runs turns and answers approvals on the Host the
bsh window is configured for: the same `settings.toml`, the same vault entry and
the same open workspace, so a terminal turn and a window turn send one request.

options:
  --data-dir <dir>   durable state directory (default <bsh data dir>/runtime)
  --workspace <dir>  directory the agent works in when this process starts the
                     Host (default the workspace bsh has open)
  --echo             use the deterministic echo executor when this process
                     starts the Host, instead of the configured model
  --session <uuid>   open this session instead of the most recent one
  --help             show this message

keys:
  Enter              send the turn to the Host
  Ctrl-N / Ctrl-P    next / previous session
  Ctrl-T             create a session
  Ctrl-C             cancel the session's running execution
  Ctrl-Y / Ctrl-R    approve / reject the oldest pending approval
  Up / Down          scroll the transcript
  Ctrl-D             quit; a Host this process started drains first - in-flight
                     work gets its shutdown grace - while a Host owned by
                     `bsh host` keeps running
";

/// Parsed `bsh host` arguments.
struct HostOptions {
    data_dir: PathBuf,
    workspace: Option<PathBuf>,
    socket: Option<PathBuf>,
    echo: bool,
}

impl HostOptions {
    fn parse(args: &[String]) -> Result<Option<Self>, String> {
        let mut options = Self {
            data_dir: default_data_dir(),
            workspace: None,
            socket: None,
            echo: false,
        };
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--help" | "-h" => return Ok(None),
                "--echo" => options.echo = true,
                "--data-dir" | "--workspace" | "--socket" => {
                    let flag = args[index].clone();
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| format!("{flag} needs a value"))?;
                    index += 1;
                    let path = PathBuf::from(value);
                    match flag.as_str() {
                        "--data-dir" => options.data_dir = path,
                        "--workspace" => options.workspace = Some(path),
                        _ => options.socket = Some(path),
                    }
                }
                other => return Err(format!("unknown argument {other}")),
            }
            index += 1;
        }
        Ok(Some(options))
    }
}

/// Run `bsh host` and return its exit code.
pub(crate) fn host_main(args: &[String]) -> i32 {
    let options = match HostOptions::parse(args) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{HOST_USAGE}");
            return 0;
        }
        Err(message) => {
            crate::runtime::report(message);
            eprintln!("{HOST_USAGE}");
            return 2;
        }
    };
    let config = match build_config(options) {
        Ok(config) => config,
        Err(message) => {
            crate::runtime::report(message);
            return 2;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            crate::runtime::report(format!("cannot start the async runtime: {error}"));
            return 1;
        }
    };
    match runtime.block_on(serve(config)) {
        Ok(()) => 0,
        Err(message) => {
            crate::runtime::report(message);
            1
        }
    }
}

fn build_config(options: HostOptions) -> Result<RuntimeConfig, String> {
    let mut config = RuntimeConfig::with_data_dir(options.data_dir).load()?;
    apply_shell_settings(&mut config, &shell_settings(options.workspace.as_deref()));
    // An explicit flag is the reader's own instruction, so it wins over both
    // documents: the shell's settings describe the default, not a ceiling.
    if let Some(workspace) = options.workspace {
        config.workspace_root = workspace;
    }
    if let Some(socket) = options.socket {
        config.socket_path = socket;
    }
    config.echo = options.echo;
    Ok(config)
}

/// Open the Host, serve until a stop signal, then drain it.
async fn serve(config: RuntimeConfig) -> Result<(), String> {
    let host = RuntimeHost::open(config.clone())
        .await
        .map_err(|error| error.to_string())?;
    // The model id is printed with the executor: the one thing a reader has to
    // be able to check is which configuration the Host took. The Host decided
    // it at open - from the echo flag, the shell's settings and `host.toml`, in
    // that order - so the banner asks the Host rather than re-deriving it here.
    let executor = host.executor_label().to_string();
    println!(
        "bsh host: instance {} serving workspace {}",
        host.instance_id(),
        host.workspace_id()
    );
    println!("          data dir {}", config.data_dir().display());
    println!("          socket   {}", config.socket_path.display());
    println!("          executor {executor}");

    let mut server = tokio::spawn(IpcServer::new(host.clone()).serve());
    let outcome = tokio::select! {
        result = &mut server => Some(result),
        signal = shutdown_signal() => {
            signal?;
            println!("bsh host: stopping");
            host.request_shutdown(ShutdownMode::Graceful);
            None
        }
    };
    // Whether the signal or the transport ended first, the Host still has to
    // finish in-flight work and record its stop before the process exits.
    host.drain().await;
    let served = match outcome {
        Some(result) => result,
        None => server.await,
    };
    match served {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(format!("transport task failed: {error}")),
    }
}

/// Resolve on the first stop request: Ctrl-C, or SIGTERM where it exists.
async fn shutdown_signal() -> Result<(), String> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|error| format!("cannot install the SIGTERM handler: {error}"))?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.map_err(|error| format!("cannot install the Ctrl-C handler: {error}"))
            }
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .map_err(|error| format!("cannot install the Ctrl-C handler: {error}"))
    }
}

/// Parsed `bsh tui` arguments.
struct TuiOptions {
    data_dir: PathBuf,
    session: Option<SessionId>,
    workspace: Option<PathBuf>,
    echo: bool,
}

impl TuiOptions {
    fn parse(args: &[String]) -> Result<Option<Self>, String> {
        let mut options = Self {
            data_dir: default_data_dir(),
            session: None,
            workspace: None,
            echo: false,
        };
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--help" | "-h" => return Ok(None),
                "--echo" => options.echo = true,
                "--data-dir" | "--session" | "--workspace" => {
                    let flag = args[index].clone();
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| format!("{flag} needs a value"))?;
                    index += 1;
                    match flag.as_str() {
                        "--data-dir" => options.data_dir = PathBuf::from(value),
                        "--workspace" => options.workspace = Some(PathBuf::from(value)),
                        _ => {
                            options.session = Some(
                                value
                                    .parse()
                                    .map_err(|error| format!("--session {value}: {error}"))?,
                            );
                        }
                    }
                }
                other => return Err(format!("unknown argument {other}")),
            }
            index += 1;
        }
        Ok(Some(options))
    }
}

/// Run `bsh tui` and return its exit code.
pub(crate) fn tui_main(args: &[String]) -> i32 {
    let options = match TuiOptions::parse(args) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{TUI_USAGE}");
            return 0;
        }
        Err(message) => {
            crate::runtime::report(message);
            eprintln!("{TUI_USAGE}");
            return 2;
        }
    };
    // The authority comes first: a client needs a Host to attach to, and when
    // the data directory already has one this call starts nothing.
    // The workspace to offer in the client's `@` list is the one the Host was
    // handed - the same root the window would open - not the shell's cwd.
    let config = tui_config(&options);
    let workspace =
        (!config.workspace_root.as_os_str().is_empty()).then(|| config.workspace_root.clone());
    let authority = match Authority::open(config) {
        Ok(authority) => authority,
        Err(error) => {
            crate::runtime::report(error);
            return 1;
        }
    };
    let outcome = crate::tui::run(&options.data_dir, options.session, workspace);
    // An embedded Host belongs to this process, so it stops after the
    // interface restored the terminal: its messages stay readable and the
    // socket is gone before the process exits.
    if let Authority::Started(host) = authority {
        host.stop();
    }
    match outcome {
        Ok(()) => 0,
        Err(error) => {
            crate::runtime::report(error);
            1
        }
    }
}

/// Configuration for a Host this process would start.
///
/// A broken `host.toml` is reported by the client that follows instead of here:
/// when another process already owns the data directory its settings, not these,
/// describe the Host that is serving.
fn tui_config(options: &TuiOptions) -> RuntimeConfig {
    let mut config = match RuntimeConfig::with_data_dir(&options.data_dir).load() {
        Ok(config) => config,
        Err(message) => {
            log::warn!("ignoring unusable Host configuration: {message}");
            RuntimeConfig::with_data_dir(&options.data_dir)
        }
    };
    apply_shell_settings(&mut config, &shell_settings(options.workspace.as_deref()));
    if let Some(workspace) = &options.workspace {
        config.workspace_root = workspace.clone();
    }
    config.echo = options.echo;
    config
}

/// The shell's own configuration, read where `bsh` reads it.
///
/// `root` is the workspace an explicit `--workspace` names: opening that folder
/// in the window would load its document, so that is the document which
/// describes the agent here. A workspace with no settings file is a fresh one
/// and resolves to the defaults, exactly as a first visit in the window would.
fn shell_settings(root: Option<&Path>) -> Settings {
    with_workspace_document(Settings::load(), root)
}

/// Replace the workspace document with the one an explicit `--workspace`
/// carries, keeping the global document as it was read.
fn with_workspace_document(mut settings: Settings, root: Option<&Path>) -> Settings {
    if let Some(root) = root {
        settings.workspace =
            WorkspaceSettings::load(&WorkspaceSettings::path(root)).unwrap_or_default();
    }
    settings
}

/// Fold the shell's effective agent configuration into the Host's.
///
/// Every field comes from the one resolution the window's own build uses:
/// [`Settings::effective`] for the model, endpoint, persona and root, and the
/// vault for the credential. The model is written in the wire form
/// `<provider>/<model>` because the engine registers one vendor per provider id
/// and dispatches on that prefix, so a Host that dropped it would call a
/// different endpoint than the window does.
///
/// Nothing resolves when the shell has no default model, and then `host.toml`
/// keeps whatever it says - a Host started from a scratch data directory with no
/// shell configuration still has one way to be configured.
fn apply_shell_settings(config: &mut RuntimeConfig, settings: &Settings) {
    let root = settings.active_workspace_root();
    if !root.trim().is_empty() {
        config.workspace_root = PathBuf::from(root);
    }
    let effective = settings.effective();
    if effective.model.trim().is_empty() || effective.base_url.trim().is_empty() {
        return;
    }
    let model = if effective.provider.trim().is_empty() {
        effective.model.clone()
    } else {
        format!("{}/{}", effective.provider, effective.model)
    };
    // A contributed provider may declare that it needs no credential - an open
    // or anonymous lane - and the build must not invent a key for it.
    let api_key = if effective.provider.trim().is_empty()
        || settings.global.provider_requires_key(&effective.provider)
    {
        let vault = crate::vault::Vault::new(settings.vault_path());
        crate::caps::provider_credential(
            &effective.provider,
            effective.api_key_vault.as_deref(),
            &vault,
        )
        .0
    } else {
        String::new()
    };
    config.provider = ReactSettings {
        model,
        base_url: effective.base_url.clone(),
        api_key,
        // A persona the shell names is the configured system prompt; when it
        // names none, a `host.toml` that names one keeps it rather than being
        // replaced by an empty string.
        system_prompt: if effective.llm_system_prompt().trim().is_empty() {
            config.provider.system_prompt.clone()
        } else {
            effective.llm_system_prompt().to_string()
        },
        // `host.toml` owns the step cap: the shell's own build runs unbounded,
        // and a zero here means the same.
        max_steps: config.provider.max_steps,
    };
}

/// The durable state directory both subcommands share.
///
/// It sits in the shell's own data root - beside `settings.toml`, under the
/// same `BSH_HOME` override - so the Host's journal, the configuration it runs
/// and the configuration the window edits are one directory rather than a
/// second home the reader has to find.
fn default_data_dir() -> PathBuf {
    crate::settings::dir().join("runtime")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::global::{GlobalSettings, ModelEntry, ModelRef};

    /// A scratch directory under the test's own name, emptied first.
    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bsh-cli-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the scratch directory is writable");
        dir
    }

    /// The shell's two documents as a launch reads them: the global file names
    /// one provider with a default model, the workspace file carries a persona.
    fn shell_home(label: &str, root: &Path) -> Settings {
        let home = scratch(label);
        let mut global = GlobalSettings::default();
        global.add_provider(
            "alpha",
            "",
            "https://alpha.example/v1",
            &[ModelEntry::named("alpha-fast")],
        );
        global.default_model = Some(ModelRef {
            provider: "alpha".to_string(),
            model: "alpha-fast".to_string(),
        });
        let mut workspace = WorkspaceSettings::default();
        workspace.agent.system_prompt = "be terse".to_string();
        let mut settings = Settings {
            global,
            workspace,
            save_root: Some(home),
            ..Settings::default()
        };
        let id = settings.note_workspace("project", &root.display().to_string());
        settings.global.general.active_workspace = id;
        settings
    }

    /// The requirement: a Host a subcommand starts sends the request the shell
    /// is configured to send - one model id, one endpoint, one persona, one
    /// workspace - rather than whatever a separate `host.toml` happens to say.
    #[test]
    fn a_host_started_by_a_subcommand_runs_the_shells_configuration() {
        let root = scratch("workspace");
        let settings = shell_home("configured", &root);
        let mut config = RuntimeConfig::with_data_dir(scratch("host"));
        apply_shell_settings(&mut config, &settings);
        // The engine dispatches on the provider prefix, so the wire id keeps it.
        assert_eq!(config.provider.model, "alpha/alpha-fast");
        assert_eq!(config.provider.base_url, "https://alpha.example/v1");
        assert!(config.provider.is_configured());
        // No vault file exists in this scratch home, so there is no credential -
        // but the Host is still configured, exactly as the window would be.
        assert!(config.provider.api_key.is_empty());
        assert_eq!(config.provider.system_prompt, "be terse");
        // The workspace is the one bsh has open, not the terminal's own cwd.
        assert_eq!(config.workspace_root, root);
    }

    /// A Host that has no shell configuration keeps `host.toml`: the settings
    /// are the default source, not a replacement that erases the other one.
    #[test]
    fn host_toml_survives_when_the_shell_names_no_model() {
        let root = scratch("unconfigured-workspace");
        let mut settings = shell_home("unconfigured", &root);
        settings.global.default_model = None;
        let mut config = RuntimeConfig::with_data_dir(scratch("toml-host"));
        config.provider = ReactSettings {
            model: "from-host-toml".to_string(),
            base_url: "https://toml.example/v1".to_string(),
            ..ReactSettings::default()
        };
        apply_shell_settings(&mut config, &settings);
        assert_eq!(config.provider.model, "from-host-toml");
        assert_eq!(config.provider.base_url, "https://toml.example/v1");
        assert_eq!(config.workspace_root, root);
    }

    /// A persona in the shell is the configured system prompt, and one in
    /// `host.toml` is not erased by a shell that names none.
    #[test]
    fn a_persona_needs_a_source_and_each_one_survives_the_other() {
        let root = scratch("persona-workspace");
        let mut settings = shell_home("persona", &root);
        settings.workspace.agent.system_prompt.clear();
        let mut config = RuntimeConfig::with_data_dir(scratch("persona-host"));
        config.provider.system_prompt = "from host.toml".to_string();
        apply_shell_settings(&mut config, &settings);
        assert_eq!(config.provider.model, "alpha/alpha-fast");
        assert_eq!(config.provider.system_prompt, "from host.toml");
    }

    /// `--workspace` names the folder the window would open, so its own document
    /// is the one that describes the agent - a checkout with no file is a fresh
    /// workspace, not an error.
    #[test]
    fn an_explicit_workspace_reads_its_own_document() {
        let root = scratch("explicit-workspace");
        let path = WorkspaceSettings::path(&root);
        std::fs::create_dir_all(path.parent().expect("a nested path"))
            .expect("the workspace settings directory is writable");
        std::fs::write(&path, "[agent]\nsystem_prompt = \"from this checkout\"\n")
            .expect("the workspace document is writable");
        let settings = with_workspace_document(Settings::default(), Some(&root));
        assert_eq!(settings.workspace.agent.system_prompt, "from this checkout");
        let absent = with_workspace_document(Settings::default(), Some(&scratch("fresh")));
        assert!(absent.workspace.agent.system_prompt.is_empty());
    }
}
