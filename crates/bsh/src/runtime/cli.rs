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
leaves it running. Otherwise it starts a standalone `bsh host` process,
serves it on the same local IPC endpoint and leaves it running on quit, so
`bsh tui` needs no second command to be a complete runtime and TUI exit never
cancels or stops Host-owned work.

The interface shows sessions, runs turns and answers approvals on the Host the
bsh window is configured for: the same `settings.toml` and the same vault entry,
so a terminal turn and a window turn send one request. The workspace is the
folder `bsh tui` was started in - the project this terminal is open on - and
--workspace names another one.

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
  Ctrl-C             not bound to job cancellation; use /cancel [confirm]
  Ctrl-Y / Ctrl-R    approve / reject the oldest pending approval
  Up / Down          scroll the transcript
  Ctrl-D             quit; a Host started by `bsh tui` keeps running, and a
                     Host owned by `bsh host` keeps running
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
    let settings = shell_settings(options.workspace.as_deref());
    apply_shell_settings(&mut config, &settings);
    // An explicit flag is the reader's own instruction, so it wins over both
    // documents: the shell's settings describe the default, not a ceiling.
    if let Some(workspace) = options.workspace {
        config.workspace_root = workspace;
    }
    // The final root, not the one the settings were read for: a `--workspace`
    // names the folder the sessions belong to.
    resolve_session_workspace(&mut config, settings);
    if let Some(socket) = options.socket {
        config.socket_path = socket;
    }
    config.echo = options.echo;
    Ok(config)
}

/// Tag the Host's sessions with the workspace the final root names.
///
/// The registry is how the window decides which sessions belong under which
/// workspace row. A Host that served a folder the registry did not know would
/// write chats the window could never list, so the folder is registered here -
/// the same act as opening it in the window - and a folder the registry already
/// knows keeps its existing identity.
fn resolve_session_workspace(config: &mut RuntimeConfig, mut settings: Settings) {
    let root = config.canonical_workspace();
    let root_text = root.display().to_string();
    if let Some(workspace) = settings.global.workspaces.iter().find(|workspace| {
        std::fs::canonicalize(&workspace.root).is_ok_and(|registered| registered == root)
    }) {
        config.session_workspace = Some(workspace.id.clone());
        config.session_workspace_name = workspace.name.clone();
        return;
    }
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| root_text.clone());
    let id = settings.note_workspace(&name, &root_text);
    // A scratch configuration keeps its registry in scratch space; a test build
    // never writes the reader's global document.
    if (settings.save_root.is_some() || !cfg!(test))
        && let Err(error) = settings.save_global()
    {
        log::warn!("the workspace {root_text} could not be registered: {error}");
    }
    config.session_workspace = Some(id);
    config.session_workspace_name = name;
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
    // The TUI is only a client. When no standalone Host owns the data
    // directory, start `bsh host` as a SEPARATE process and attach to it; a
    // terminal exit never stops Host-owned work. `bsh host` remains explicitly
    // startable for a Host that should outlive any particular client.
    let _standalone = match spawn_standalone_host(&config) {
        Ok(child) => child,
        Err(error) => {
            crate::runtime::report(error);
            return 1;
        }
    };
    let outcome = crate::tui::run(&options.data_dir, options.session, workspace);
    // Do not stop `_standalone`: Host-owned work survives TUI exit. A child
    // Host dropped here is not killed; it keeps serving the data directory.
    match outcome {
        Ok(()) => 0,
        Err(error) => {
            crate::runtime::report(error);
            1
        }
    }
}

/// Start a standalone `bsh host` process when no Host already serves `config`.
///
/// The returned child is intentionally not killed on TUI exit. Client disconnect
/// is not Host shutdown and not Job cancellation.
fn spawn_standalone_host(config: &RuntimeConfig) -> Result<Option<std::process::Child>, String> {
    if crate::runtime::endpoint::Endpoint::discover(&config.data_dir).is_ok() {
        return Ok(None);
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot find the bsh executable: {error}"))?;
    let mut command = std::process::Command::new(executable);
    command
        .arg("host")
        .arg("--data-dir")
        .arg(&config.data_dir)
        .arg("--workspace")
        .arg(&config.workspace_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if config.echo {
        command.arg("--echo");
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start standalone Runtime Host: {error}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while crate::runtime::endpoint::Endpoint::discover(&config.data_dir).is_err() {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot query standalone Runtime Host: {error}"))?
        {
            return Err(format!(
                "standalone Runtime Host exited during startup: {status}"
            ));
        }
        if std::time::Instant::now() >= deadline {
            return Err("standalone Runtime Host did not publish its endpoint".to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(Some(child))
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
    // The terminal runs where it was started: the folder `bsh tui` is invoked
    // from is the project, so its sessions, its `@` list, its shell and its own
    // `.bsh` document are that folder's - the way opening it in the window
    // makes it the workspace. `--workspace` still names the folder outright, and
    // only a shell with no readable cwd falls back to the one the desktop last
    // had open.
    let root = terminal_workspace(options.workspace.clone(), std::env::current_dir().ok());
    let settings = shell_settings(root.as_deref());
    apply_shell_settings(&mut config, &settings);
    if let Some(root) = root {
        config.workspace_root = root;
    }
    // The terminal serves the folder it was started in, so its chats are that
    // folder's project chats: the same registration `open this folder` makes in
    // the window, and the reason a chat made here appears in its sidebar.
    resolve_session_workspace(&mut config, settings);
    config.echo = options.echo;
    config
}

/// The folder a terminal Host serves.
///
/// An explicit `--workspace` is the reader naming the folder, so it wins. A
/// terminal started without one serves the directory it was started in: a
/// shell in a checkout is the same statement the window's picker makes, and a
/// Host that served some other folder would run every turn somewhere the
/// reader is not looking. `None` leaves the decision to the shell's settings,
/// which is the only remaining source when the cwd cannot be read.
fn terminal_workspace(explicit: Option<PathBuf>, cwd: Option<PathBuf>) -> Option<PathBuf> {
    explicit.or(cwd).filter(|root| !root.as_os_str().is_empty())
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
    // The session store is the desktop's, so the Host takes the desktop's
    // workspace identity, its bin and the credentials a session document must
    // never carry. These are read before the model check below: a Host with no
    // model still owns sessions, and a session tagged with a derived id would
    // be invisible to the window's workspace filter.
    if let Some(workspace) = settings.active_workspace() {
        config.session_workspace = Some(workspace.id.clone());
        config.session_workspace_name = workspace.name.clone();
    }
    config.session_secrets = settings.secrets();
    // `trash_dir` refuses to invent a path for a test that has no save root, so
    // a scratch configuration keeps its bin under the Host's data directory.
    if settings.save_root.is_some() || !cfg!(test) {
        config.session_trash_dir = Some(settings.trash_dir());
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
        // For `bsh host` the workspace is the one bsh has open: a detached
        // authority serves the desktop's project until a client names another.
        assert_eq!(config.workspace_root, root);
    }

    /// `bsh tui` serves the folder it was started in, so a terminal opened in a
    /// checkout runs its turns there instead of in whatever workspace the
    /// window last had open. `--workspace` still names a folder outright, and a
    /// shell whose cwd cannot be read keeps the configured one.
    #[test]
    fn the_terminal_serves_the_folder_it_was_started_in() {
        let here = scratch("terminal-cwd");
        let named = scratch("terminal-named");
        assert_eq!(terminal_workspace(None, Some(here.clone())), Some(here));
        // An explicit flag outranks the directory, so a client can point at a
        // project it is not standing in.
        assert_eq!(
            terminal_workspace(Some(named.clone()), Some(scratch("terminal-elsewhere"))),
            Some(named)
        );
        // No readable directory is the one case that defers to the settings.
        assert_eq!(terminal_workspace(None, None), None);
        // An empty path is no path: it must not become the process root.
        assert_eq!(
            terminal_workspace(Some(PathBuf::new()), Some(PathBuf::new())),
            None
        );
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

    /// A folder the registry does not know is registered once, and a folder it
    /// knows keeps the identity (and name) the window gave it. This is what
    /// makes a chat the terminal creates appear under the window's row.
    #[test]
    fn the_served_folder_is_registered_with_a_stable_identity() {
        let root = scratch("served-folder");
        let mut settings = Settings {
            save_root: Some(scratch("register-home")),
            ..Settings::default()
        };
        let mut first = RuntimeConfig::with_data_dir(scratch("first-host"));
        first.workspace_root = root.clone();
        resolve_session_workspace(&mut first, settings.clone());
        let id = first.session_workspace.clone().expect("a workspace id");
        assert_eq!(
            settings.global.workspaces.len(),
            0,
            "the clone is untouched"
        );
        assert!(
            settings
                .global
                .workspaces
                .iter()
                .all(|workspace| workspace.id != id),
            "the original settings did not observe the clone's registration"
        );

        // Registering through the API and resolving again keeps that id.
        let named = settings.note_workspace("my project", &root.display().to_string());
        let mut second = RuntimeConfig::with_data_dir(scratch("second-host"));
        second.workspace_root = root;
        resolve_session_workspace(&mut second, settings);
        assert_eq!(second.session_workspace.as_deref(), Some(named.as_str()));
        assert_eq!(second.session_workspace_name, "my project");
    }
}
