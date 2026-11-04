//! The external-process runtime: JSON-RPC 2.0 over stdin/stdout.
//!
//! A process package is the right shape for a language service, an MCP-style
//! server or an integration with dependencies that cannot live in wasm. The
//! host starts the executable with a cleared environment, a package-root
//! working directory, piped stdio and a deadline; it never goes through a
//! shell. The wire is one JSON object per line, with an explicit initialize
//! handshake and a tools/call request whose result is the tool's JSON value.

use std::collections::{BTreeMap, VecDeque};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
#[cfg(unix)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::contract::{load_tool_descriptors, CallContext, Runtime, ToolDescriptor, ToolResult};
use ferris_core::capability::{CallScope, Capability, PolicyEngine};
use ferris_core::events::{EventBus, EventFilter, ExtensionEvent, Subscription};
use ferris_core::manifest::{is_allowlisted_command, validate_relative_path, Manifest};
use ferris_core::types::{
    ContributionId, ExtensionError, ExtensionId, RuntimeKind, TrustLevel, HOST_VERSION,
};

/// The protocol generation a process must speak.
pub const PROTOCOL_VERSION: &str = "1.0";

/// The largest one-line request or response.
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// The initialize handshake deadline.
pub const HANDSHAKE_MS: u64 = 10_000;

/// The shutdown deadline.
pub const SHUTDOWN_MS: u64 = 1_000;

/// One live child process.
#[cfg(not(windows))]
type ProcessChild = Child;
#[cfg(windows)]
type ProcessChild = heel::Child;

#[cfg(not(windows))]
type ProcessStdin = ChildStdin;
#[cfg(windows)]
type ProcessStdin = tokio::fs::File;

#[cfg(not(windows))]
type ProcessStdout = ChildStdout;
#[cfg(windows)]
type ProcessStdout = tokio::fs::File;

struct ProcessSession {
    child: ProcessChild,
    stdin: ProcessStdin,
    stdout: BufReader<ProcessStdout>,
    next_id: u64,
    token: String,
}

/// Kill the session child through the platform handle.
fn start_kill(child: &mut ProcessChild) {
    #[cfg(not(windows))]
    let _ = child.start_kill();
    #[cfg(windows)]
    let _ = child.kill();
}

/// Wait for the session child, ignoring the exit status.
async fn wait_child(child: &mut ProcessChild) {
    let _ = child.wait().await;
}

/// The child pid, when the platform handle exposes one.
fn child_id(child: &ProcessChild) -> Option<u32> {
    #[cfg(not(windows))]
    {
        child.id()
    }
    #[cfg(windows)]
    {
        Some(child.id())
    }
}

impl ProcessSession {
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
        extension: &ExtensionId,
    ) -> Result<Value, ExtensionError> {
        self.request_with_progress(method, params, timeout_ms, extension, None)
            .await
    }

    async fn request_with_progress(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
        extension: &ExtensionId,
        progress: Option<&tokio::sync::mpsc::UnboundedSender<Value>>,
    ) -> Result<Value, ExtensionError> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_vec(&request).map_err(|error| ExtensionError::Protocol {
            extension: extension.to_string(),
            message: error.to_string(),
        })?;
        if line.len() > MAX_LINE_BYTES {
            return Err(ExtensionError::Limit {
                extension: extension.to_string(),
                message: format!(
                    "request is {} bytes; the line ceiling is {MAX_LINE_BYTES}",
                    line.len()
                ),
            });
        }
        line.push(b'\n');
        let stdin = &mut self.stdin;
        let stdout = &mut self.stdout;
        let deadline = Duration::from_millis(timeout_ms.max(1));
        let exchange = async {
            stdin
                .write_all(&line)
                .await
                .map_err(|source| ExtensionError::Io {
                    path: PathBuf::from("<process stdin>"),
                    source,
                })?;
            stdin.flush().await.map_err(|source| ExtensionError::Io {
                path: PathBuf::from("<process stdin>"),
                source,
            })?;
            loop {
                let Some(response) = read_limited_line(stdout, MAX_LINE_BYTES, extension).await?
                else {
                    return Err(ExtensionError::Protocol {
                        extension: extension.to_string(),
                        message: "process closed stdout before answering".to_string(),
                    });
                };
                let line = response.trim_ascii();
                let value: Value =
                    serde_json::from_slice(line).map_err(|error| ExtensionError::Protocol {
                        extension: extension.to_string(),
                        message: format!("response is not JSON: {error}"),
                    })?;
                if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                    return Err(ExtensionError::Protocol {
                        extension: extension.to_string(),
                        message: "response is not JSON-RPC 2.0".to_string(),
                    });
                }
                if json_depth_ok(&value, 0).is_err() {
                    return Err(ExtensionError::Protocol {
                        extension: extension.to_string(),
                        message: "response nests deeper than 32 levels".to_string(),
                    });
                }
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    // A notification or an earlier response. Progress
                    // notifications are forwarded; everything else is skipped.
                    if let Some(sender) = progress {
                        if value.get("method").and_then(Value::as_str) == Some("$/progress") {
                            if let Some(params) = value.get("params") {
                                let _ = sender.send(params.clone());
                            }
                        }
                    }
                    continue;
                }
                if let Some(error) = value.get("error") {
                    // A decoded JSON-RPC error is an application-level answer,
                    // not a broken pipe: the session stays usable.
                    return Err(ExtensionError::Remote {
                        extension: extension.to_string(),
                        message: jsonrpc_error_message(error),
                    });
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
        };
        match tokio::time::timeout(deadline, exchange).await {
            Ok(result) => result,
            Err(_) => Err(ExtensionError::Timeout {
                extension: extension.to_string(),
                operation: method.to_string(),
                millis: timeout_ms,
            }),
        }
    }

    async fn notify(
        &mut self,
        method: &str,
        params: Value,
        extension: &ExtensionId,
    ) -> Result<(), ExtensionError> {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let mut line =
            serde_json::to_vec(&notification).map_err(|error| ExtensionError::Protocol {
                extension: extension.to_string(),
                message: error.to_string(),
            })?;
        if line.len() > MAX_LINE_BYTES {
            return Err(ExtensionError::Limit {
                extension: extension.to_string(),
                message: "event notification exceeds the line ceiling".to_string(),
            });
        }
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .map_err(|source| ExtensionError::Io {
                path: PathBuf::from("<process stdin>"),
                source,
            })?;
        self.stdin
            .flush()
            .await
            .map_err(|source| ExtensionError::Io {
                path: PathBuf::from("<process stdin>"),
                source,
            })
    }
}

/// Attach the child's recent stderr to a protocol failure.
///
/// "process closed stdout before answering" is true but not actionable; the
/// line the interpreter or sandbox printed on stderr is what names the cause.
fn with_stderr_tail(error: ExtensionError, tail: &StdMutex<VecDeque<String>>) -> ExtensionError {
    let lines = {
        let tail = tail.lock().expect("stderr tail lock");
        tail.iter().cloned().collect::<Vec<_>>().join(" | ")
    };
    if lines.is_empty() {
        return error;
    }
    match error {
        ExtensionError::Protocol { extension, message } => ExtensionError::Protocol {
            extension,
            message: format!("{message} (stderr: {lines})"),
        },
        other => other,
    }
}

/// How often the watchdog samples resident memory.
#[cfg(unix)]
const MEMORY_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

/// How many consecutive sampling failures are tolerated before the child is
/// treated as unobservable and terminated.
#[cfg(unix)]
const MEMORY_SAMPLE_RETRIES: u32 = 8;

/// A live memory watchdog.
///
/// The sampler runs on its own thread, not a Tokio worker: sampling calls
/// procfs or forks ps, which must never block an executor. Stopping only
/// signals the flag; the thread exits at its next tick.
#[cfg(unix)]
pub(crate) struct MemoryWatchdog {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl MemoryWatchdog {
    /// Ask the sampler to exit. It never signals a different process: identity
    /// is re-read before every kill.
    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(unix)]
impl Drop for MemoryWatchdog {
    fn drop(&mut self) {
        self.stop();
        self.handle.take();
    }
}

/// Kill a child whose resident memory exceeds its manifest ceiling.
///
/// RLIMIT_AS is not honoured by macOS, so the host samples RSS and sends
/// SIGKILL itself; Linux keeps the rlimit as the first line and the sampler as
/// the same backstop. The sampler is bound to the child's start-time identity,
/// so a pid recycled after the child exits is never signalled, and a sampling
/// failure that persists while the child is alive terminates it (fail closed).
#[cfg(unix)]
pub(crate) fn spawn_memory_watchdog(
    pid: u32,
    limit: u64,
    identity: Option<String>,
) -> MemoryWatchdog {
    let stop = Arc::new(AtomicBool::new(false));
    let token = Arc::clone(&stop);
    if identity.is_none() {
        log::warn!(
            target: "bsh::extension",
            "memory watchdog for pid {pid} could not read a start-time identity; it will not signal the child"
        );
    }
    let handle = (limit > 0).then(|| {
        std::thread::Builder::new()
            .name(format!("bsh-memory-{pid}"))
            .spawn(move || {
                let mut unavailable = 0u32;
                while !token.load(Ordering::SeqCst) {
                    std::thread::sleep(MEMORY_SAMPLE_INTERVAL);
                    if token.load(Ordering::SeqCst) {
                        break;
                    }
                    match process_rss_bytes(pid) {
                        Some(rss) if rss > limit => {
                            if identity_matches(pid, identity.as_deref()) {
                                signal_kill(pid);
                            } else {
                                log::warn!(
                                    target: "bsh::extension",
                                    "pid {pid} exceeded its memory ceiling but is no longer the child that was spawned; not signalling"
                                );
                            }
                            break;
                        }
                        Some(_) => unavailable = 0,
                        None => {
                            // A missing process is not a sampling failure: the
                            // child exited and the pid may already be reused.
                            if !process_exists(pid) {
                                break;
                            }
                            unavailable += 1;
                            if unavailable >= MEMORY_SAMPLE_RETRIES {
                                log::warn!(
                                    target: "bsh::extension",
                                    "pid {pid} could not be sampled for {}s while alive; terminating it",
                                    MEMORY_SAMPLE_RETRIES as u64 * MEMORY_SAMPLE_INTERVAL.as_millis() as u64 / 1000
                                );
                                if identity_matches(pid, identity.as_deref()) {
                                    signal_kill(pid);
                                }
                                break;
                            }
                        }
                    }
                }
            })
            .ok()
    });
    MemoryWatchdog {
        stop,
        handle: handle.flatten(),
    }
}

/// Whether the pid still names the process that was spawned.
#[cfg(unix)]
fn identity_matches(pid: u32, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return false;
    };
    child_identity(pid).is_some_and(|current| current == expected)
}

/// A start-time marker that distinguishes the spawned child from a pid reuse.
#[cfg(unix)]
pub(crate) fn child_identity(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let start = stat_start_time(&text)?;
        return Some(format!("linux:{start}"));
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/bin/ps")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let text = text.trim();
        return (!text.is_empty()).then(|| format!("macos:{text}"));
    }
    #[allow(unreachable_code)]
    None
}

/// The start time (field 22) in a /proc/<pid>/stat line.
///
/// The comm field may contain spaces and parentheses, so the parse starts after
/// the last ')' rather than splitting from the beginning.
#[cfg(any(target_os = "linux", test))]
fn stat_start_time(text: &str) -> Option<u64> {
    let close = text.rfind(')')?;
    let rest = text.get(close + 1..)?.trim_start();
    // Field 3 is the first token after comm; start time is field 22.
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// Resident bytes from a Linux statm line.
///
/// Field 1 is total virtual pages; only field 2 is resident memory, and the
/// page size is the system's, not a hard-coded 4 KiB.
#[cfg(any(target_os = "linux", test))]
fn statm_rss_bytes(text: &str, page_size: u64) -> Option<u64> {
    let pages: u64 = text.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages.saturating_mul(page_size))
}

#[cfg(target_os = "linux")]
fn page_size_bytes() -> Option<u64> {
    // SAFETY: sysconf is side-effect free and reports a process-wide constant.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    (size > 0).then_some(size as u64)
}

/// Whether a pid still names a live process.
#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    // SAFETY: signal 0 performs only an existence/permission check.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Terminate one process. Callers verify the start-time identity first.
#[cfg(unix)]
fn signal_kill(pid: u32) {
    // SAFETY: the caller has verified pid still names the spawned child.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

/// Resident memory of one child, from procfs on Linux or ps on macOS.
fn process_rss_bytes(pid: u32) -> Option<u64> {
    #[cfg(not(unix))]
    let _ = pid;
    #[cfg(target_os = "linux")]
    {
        let page_size = page_size_bytes()?;
        let text = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        return statm_rss_bytes(&text, page_size);
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/bin/ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        let kb: u64 = text.trim().parse().ok()?;
        return Some(kb.saturating_mul(1024));
    }
    #[allow(unreachable_code)]
    None
}

/// A bubblewrap plan that gives the child a private namespace, a read-only
/// root, and writable binds only for the package, workspace, data and temp.
///
/// Kept platform-independent so the argument plan is testable on macOS even
/// though it is only executed on Linux.
pub fn linux_bwrap_plan(
    root: &Path,
    workspace: Option<&Path>,
    data_root: Option<&Path>,
    program: &Path,
    args: &[String],
) -> Vec<String> {
    let mut plan = vec![
        "--die-with-parent".to_string(),
        // Keep the network namespace private; process extensions do not
        // implicitly inherit host network access.
        "--unshare-all".to_string(),
        "--ro-bind".to_string(),
        "/".to_string(),
        "/".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        // The read-only root is a runtime filesystem view, not a source of
        // host-user data. Hide common credential/config locations.
        "--tmpfs".to_string(),
        "/home".to_string(),
        "--tmpfs".to_string(),
        "/root".to_string(),
        "--tmpfs".to_string(),
        "/run".to_string(),
        "--tmpfs".to_string(),
        "/tmp".to_string(),
    ];
    // Bubblewrap mount and chdir arguments must be absolute. Package roots
    // commonly arrive as relative paths from `bsh ext mock` and development CLI.
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut writable = vec![root.clone()];
    if let Some(workspace) = workspace {
        writable.push(
            workspace
                .canonicalize()
                .unwrap_or_else(|_| workspace.to_path_buf()),
        );
    }
    if let Some(data_root) = data_root {
        writable.push(
            data_root
                .canonicalize()
                .unwrap_or_else(|_| data_root.to_path_buf()),
        );
    }
    // Mount parent directories before children so nested writable mounts do
    // not hide one another (for example, a package inside the workspace).
    writable.sort_by_key(|path| path.components().count());
    writable.dedup();

    let mut bound = Vec::<PathBuf>::new();
    let mut created_hidden_dirs = Vec::<PathBuf>::new();
    let hidden_roots = [
        Path::new("/tmp"),
        Path::new("/home"),
        Path::new("/root"),
        Path::new("/run"),
    ];
    for path in writable {
        if hidden_roots.contains(&path.as_path())
            || bound.iter().any(|parent| path.starts_with(parent))
        {
            continue;
        }

        // Recreate mount destinations below tmpfs-hidden roots. This permits
        // an explicitly declared package/workspace without exposing sibling
        // files from the host user's home directory.
        if let Some(hidden_root) = hidden_roots
            .iter()
            .find(|hidden_root| path.starts_with(*hidden_root) && path.as_path() != **hidden_root)
        {
            if let Ok(relative) = path.strip_prefix(*hidden_root) {
                let mut destination = hidden_root.to_path_buf();
                for component in relative.components() {
                    if let std::path::Component::Normal(name) = component {
                        destination.push(name);
                        if !created_hidden_dirs.contains(&destination) {
                            plan.push("--dir".to_string());
                            plan.push(destination.display().to_string());
                            created_hidden_dirs.push(destination.clone());
                        }
                    }
                }
            }
        }

        plan.push("--bind".to_string());
        plan.push(path.display().to_string());
        plan.push(path.display().to_string());
        bound.push(path);
    }
    // A tmpfs hides these roots but is still writable, so a child could create
    // files under its host HOME path. The sandbox promises writes only inside
    // the package, workspace, data root and temp: remount the hidden roots
    // read-only after the explicit binds, which remain writable as separate
    // mounts underneath them.
    for hidden in ["/home", "/root", "/run"] {
        plan.push("--remount-ro".to_string());
        plan.push(hidden.to_string());
    }
    plan.push("--chdir".to_string());
    plan.push(root.display().to_string());
    plan.push(program.display().to_string());
    plan.extend(args.iter().cloned());
    plan
}

/// A macOS profile that allows reads everywhere and writes only inside the
/// package, the open workspace, temp and device nodes.
///
/// This is a filesystem profile, not a network sandbox: the process protocol
/// has no host network bridge, so network policy for a process package remains
/// the capability decision that authorized spawning it.
pub fn sandbox_profile(root: &Path, workspace: Option<&Path>, data_root: Option<&Path>) -> String {
    let mut profile = "(version 1)(allow default)(deny file-write*)(allow file-write*".to_string();
    let mut paths: Vec<PathBuf> = vec![
        root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/dev"),
        PathBuf::from("/private/dev"),
    ];
    if let Some(workspace) = workspace {
        paths.push(
            workspace
                .canonicalize()
                .unwrap_or_else(|_| workspace.to_path_buf()),
        );
    }
    if let Some(data_root) = data_root {
        paths.push(
            data_root
                .canonicalize()
                .unwrap_or_else(|_| data_root.to_path_buf()),
        );
    }
    for path in paths {
        let escaped = path.display().to_string().replace('"', "\\\"");
        profile.push_str(&format!(" (subpath \"{escaped}\")"));
    }
    profile.push(')');
    // The Linux plan uses --unshare-all, which includes the network namespace;
    // macOS has no equivalent, so deny network explicitly to keep the two
    // platforms' process sandboxes comparable. A future network-capable
    // process runtime must add a scoped allow rather than relying on this one.
    profile.push_str("(deny network*)");
    profile
}

/// The child-side hook that installs the manifest's resource ceilings.
///
/// RLIMIT_AS bounds virtual address space (not RSS), RLIMIT_NOFILE limits
/// descriptor exhaustion, and RLIMIT_CPU is deliberately left unlimited:
/// `max_execution_ms` is a per-call wall-clock deadline, and deriving a
/// process-lifetime CPU budget from it killed long-lived language servers after
/// a few calls. The per-call deadline is enforced by the host. Every requested
/// limit and the process-group setup are mandatory: a failure aborts exec
/// rather than launching a child with weaker-than-requested restrictions.
///
/// The host builds two kinds of command - tokio's for the process runtime and
/// the standard library's for the MCP client - and `pre_exec` is spelled the
/// same on both but they are unrelated types. The closures therefore live here
/// once, and each caller registers one on its own command.
/// The resource argument's integer type. glibc declares it as `unsigned int`
/// and types the RLIMIT_* constants the same way, while macOS and musl use
/// `int`. Naming the platform's type keeps the constants, the libc function
/// pointer and the test hooks in agreement without a cast at every call.
#[cfg(all(unix, target_env = "gnu"))]
type RlimitResource = libc::c_uint;
#[cfg(all(unix, not(target_env = "gnu")))]
type RlimitResource = libc::c_int;

#[cfg(unix)]
type SetRlimit = unsafe extern "C" fn(RlimitResource, *const libc::rlimit) -> libc::c_int;

#[cfg(unix)]
type SetPgid = unsafe extern "C" fn(libc::pid_t, libc::pid_t) -> libc::c_int;

#[cfg(unix)]
unsafe fn apply_unix_limits(
    memory: u64,
    cpu: u64,
    setrlimit: SetRlimit,
    setpgid: SetPgid,
) -> std::io::Result<()> {
    #[cfg(not(target_os = "linux"))]
    let _ = memory;
    #[cfg(target_os = "linux")]
    if memory > 0 {
        let memory = memory.max(16 * 1024 * 1024) as libc::rlim_t;
        let address = libc::rlimit {
            rlim_cur: memory,
            rlim_max: memory,
        };
        if unsafe { setrlimit(libc::RLIMIT_AS, &address) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    // `max_execution_ms` is a per-call wall-clock ceiling, not a lifetime CPU
    // budget. Deriving RLIMIT_CPU from it killed a long-lived language server
    // after a handful of calls, so the lifetime CPU limit stays unlimited; the
    // host already enforces the per-call deadline.
    let _ = cpu;
    let cpu = libc::RLIM_INFINITY;
    let cpu_limit = libc::rlimit {
        rlim_cur: cpu,
        rlim_max: cpu,
    };
    if unsafe { setrlimit(libc::RLIMIT_CPU, &cpu_limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }

    let descriptors = libc::rlimit {
        rlim_cur: 256,
        rlim_max: 256,
    };
    if unsafe { setrlimit(libc::RLIMIT_NOFILE, &descriptors) } != 0 {
        return Err(std::io::Error::last_os_error());
    }

    if unsafe { setpgid(0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(unix)]
fn rlimit_hook(
    memory: u64,
    cpu: u64,
) -> impl FnMut() -> std::io::Result<()> + Send + Sync + 'static {
    move || {
        // SAFETY: pre_exec runs in the forked child before exec. The helper
        // only calls async-signal-safe setrlimit/setpgid and captures integers.
        unsafe { apply_unix_limits(memory, cpu, libc::setrlimit, libc::setpgid) }
    }
}

/// Apply the manifest's resource ceilings to a tokio command.
pub(super) fn configure_process_limits(command: &mut Command, memory: u64, cpu: u64) {
    #[cfg(unix)]
    {
        // SAFETY: `rlimit_hook` runs in the forked child before exec and only
        // calls async-signal-safe functions.
        unsafe {
            command.pre_exec(rlimit_hook(memory, cpu));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (command, memory, cpu);
    }
}

/// Apply the same ceilings to the MCP client's synchronous command.
pub fn configure_std_process_limits(command: &mut std::process::Command, memory: u64, cpu: u64) {
    #[cfg(unix)]
    {
        // SAFETY: `rlimit_hook` runs in the forked child before exec and only
        // calls async-signal-safe functions.
        unsafe {
            command.pre_exec(rlimit_hook(memory, cpu));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (command, memory, cpu);
    }
}

fn jsonrpc_error_message(error: &Value) -> String {
    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    format!("JSON-RPC error {code}: {message}")
}

/// Read one newline-terminated line with a byte ceiling.
/// Reject a JSON value nested more than 32 levels deep.
fn json_depth_ok(value: &Value, depth: usize) -> Result<(), ()> {
    if depth > 32 {
        return Err(());
    }
    match value {
        Value::Array(items) => items
            .iter()
            .try_for_each(|item| json_depth_ok(item, depth + 1)),
        Value::Object(map) => map
            .values()
            .try_for_each(|item| json_depth_ok(item, depth + 1)),
        _ => Ok(()),
    }
}

async fn read_limited_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
    extension: &ExtensionId,
) -> Result<Option<Vec<u8>>, ExtensionError> {
    let mut line = Vec::new();
    loop {
        let mut consumed = 0usize;
        let mut complete = false;
        let mut eof = false;
        {
            let buffer = reader
                .fill_buf()
                .await
                .map_err(|source| ExtensionError::Io {
                    path: PathBuf::from("<process stdout>"),
                    source,
                })?;
            if buffer.is_empty() {
                eof = true;
            } else if let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
                line.extend_from_slice(&buffer[..=index]);
                consumed = index + 1;
                complete = true;
            } else {
                line.extend_from_slice(buffer);
                consumed = buffer.len();
            }
        }
        if consumed > 0 {
            reader.consume(consumed);
        }
        if eof {
            if line.is_empty() {
                return Ok(None);
            }
            if line.len() > limit {
                return Err(ExtensionError::Limit {
                    extension: extension.to_string(),
                    message: format!("process line exceeds {limit} bytes"),
                });
            }
            return Ok(Some(line));
        }
        if complete {
            if line.len() > limit {
                return Err(ExtensionError::Limit {
                    extension: extension.to_string(),
                    message: format!("process line exceeds {limit} bytes"),
                });
            }
            return Ok(Some(line));
        }
        if line.len() > limit {
            return Err(ExtensionError::Limit {
                extension: extension.to_string(),
                message: format!("process line exceeds {limit} bytes"),
            });
        }
    }
}

/// Consume and discard the remainder of an over-limit line, through its
/// newline, without buffering it. Returns false at EOF.
async fn discard_to_newline<R: AsyncBufRead + Unpin>(reader: &mut R) -> std::io::Result<bool> {
    loop {
        let (length, newline) = {
            let buffer = reader.fill_buf().await?;
            if buffer.is_empty() {
                return Ok(false);
            }
            match buffer.iter().position(|byte| *byte == b'\n') {
                Some(index) => (index + 1, true),
                None => (buffer.len(), false),
            }
        };
        reader.consume(length);
        if newline {
            return Ok(true);
        }
    }
}

/// One stderr line read under a hard ceiling.
enum StderrLine {
    /// A complete line, at or under the ceiling.
    Line(Vec<u8>),
    /// The line exceeded the ceiling and was discarded through its newline.
    TooLong,
    /// End of stream.
    Eof,
}

/// Read one stderr line without buffering past the ceiling, discarding an
/// over-long line so the pipe stays drained.
async fn read_stderr_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> std::io::Result<StderrLine> {
    let mut line = Vec::new();
    loop {
        let mut consumed = 0usize;
        let mut complete = false;
        let mut eof = false;
        let mut too_long = false;
        {
            let buffer = reader.fill_buf().await?;
            let room = limit.saturating_sub(line.len());
            if buffer.is_empty() {
                eof = true;
            } else if let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
                let take = index + 1;
                consumed = take;
                complete = true;
                if take > room {
                    line.extend_from_slice(&buffer[..room]);
                    too_long = true;
                } else {
                    line.extend_from_slice(&buffer[..take]);
                }
            } else if buffer.len() > room {
                line.extend_from_slice(&buffer[..room]);
                consumed = buffer.len();
                too_long = true;
            } else {
                line.extend_from_slice(buffer);
                consumed = buffer.len();
            }
        }
        if consumed > 0 {
            reader.consume(consumed);
        }
        if too_long {
            // A complete line was consumed through its newline; only an
            // unterminated tail still needs discarding.
            if !complete {
                discard_to_newline(reader).await?;
            }
            return Ok(StderrLine::TooLong);
        }
        if eof {
            return Ok(if line.is_empty() {
                StderrLine::Eof
            } else {
                StderrLine::Line(line)
            });
        }
        if complete {
            return Ok(StderrLine::Line(line));
        }
    }
}

/// The process runtime for one package.
pub struct ProcessRuntime {
    extension: ExtensionId,
    root: PathBuf,
    entry: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    tools: Vec<ToolDescriptor>,
    commands: Vec<ContributionId>,
    max_execution_ms: u64,
    max_output_bytes: u64,
    memory_bytes: u64,
    cpu_seconds: u64,
    config: String,
    workspace: Option<PathBuf>,
    data_root: Option<PathBuf>,
    policy: Option<(Arc<PolicyEngine>, TrustLevel)>,
    sandbox: bool,
    child_pid: AtomicU32,
    session: Mutex<Option<ProcessSession>>,
    subscriptions: std::sync::Mutex<Vec<Subscription>>,
    /// Bound to the spawned child's identity and cancelled on every stop path.
    #[cfg(unix)]
    watchdog: StdMutex<Option<MemoryWatchdog>>,
    /// The AppContainer child processes run in. Created on first spawn; a
    /// failure is fatal, never a direct-exec fallback.
    #[cfg(windows)]
    appcontainer:
        tokio::sync::Mutex<Option<std::sync::Arc<crate::windows_sandbox::WindowsSandbox>>>,
}

impl ProcessRuntime {
    /// Prepare a process runtime. This validates the descriptor files and the
    /// entry's path shape but does not spawn; the first call does.
    pub fn new(manifest: &Manifest, root: &Path) -> Result<Self, ExtensionError> {
        if manifest.runtime.kind != RuntimeKind::Process {
            return Err(ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: "the process runtime needs runtime.kind = \"process\"".to_string(),
            });
        }
        let entry = manifest
            .runtime
            .entry
            .clone()
            .ok_or_else(|| ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: "the process runtime needs runtime.entry".to_string(),
            })?;
        if !is_allowlisted_command(&entry) {
            validate_relative_path(&entry).map_err(|reason| ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: format!("runtime.entry: {reason}"),
            })?;
        }
        let tools = load_tool_descriptors(root, manifest)?;
        Ok(Self {
            extension: manifest.id().clone(),
            root: root.to_path_buf(),
            entry,
            args: manifest.runtime.args.clone(),
            env: manifest.runtime.env.clone(),
            tools,
            commands: manifest.contributes.commands.clone(),
            max_execution_ms: manifest.resources.execution_ms(),
            max_output_bytes: manifest.resources.output_bytes(),
            memory_bytes: manifest.resources.memory_mb().saturating_mul(1024 * 1024),
            cpu_seconds: (manifest.resources.execution_ms() / 1000).max(1) + 2,
            config: "{}".to_string(),
            workspace: None,
            data_root: None,
            policy: None,
            // Process extensions are untrusted code: require the platform OS sandbox by default.
            // On Linux, a missing/unsupported bubblewrap must fail the spawn rather than fall back.
            sandbox: cfg!(any(
                target_os = "linux",
                target_os = "macos",
                target_os = "windows"
            )),
            child_pid: AtomicU32::new(0),
            session: Mutex::new(None),
            subscriptions: std::sync::Mutex::new(Vec::new()),
            #[cfg(unix)]
            watchdog: StdMutex::new(None),
            #[cfg(windows)]
            appcontainer: tokio::sync::Mutex::new(None),
        })
    }

    /// Start a sampler bound to this child, replacing any previous one.
    #[cfg(unix)]
    fn start_watchdog(&self, pid: u32) {
        self.stop_watchdog();
        let identity = child_identity(pid);
        let watchdog = spawn_memory_watchdog(pid, self.memory_bytes, identity);
        if let Ok(mut slot) = self.watchdog.lock() {
            *slot = Some(watchdog);
        }
    }

    /// Cancel the sampler. Called before every child exit path, so a stopped
    /// child's pid is never sampled (or signalled) again.
    fn stop_watchdog(&self) {
        #[cfg(unix)]
        if let Ok(mut slot) = self.watchdog.lock() {
            if let Some(watchdog) = slot.take() {
                watchdog.stop();
            }
        }
    }

    /// The tool descriptors, without spawning the process.
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    /// Attach the effective configuration sent in the initialize handshake.
    pub fn with_config(mut self, config: String) -> Self {
        self.config = config;
        self
    }

    /// Attach the policy engine and trust level used for per-spawn checks.
    pub fn with_policy(mut self, policy: Arc<PolicyEngine>, trust: TrustLevel) -> Self {
        self.policy = Some((policy, trust));
        self
    }

    /// Attach the workspace a sandboxed child may write inside.
    pub fn with_workspace(mut self, workspace: Option<PathBuf>) -> Self {
        self.workspace = workspace;
        self
    }

    /// Attach the package data namespace a sandboxed child may write inside.
    pub fn with_data_root(mut self, data_root: Option<PathBuf>) -> Self {
        self.data_root = data_root;
        self
    }

    /// Disable or enable the platform process sandbox (tests may opt out).
    pub fn with_sandbox(mut self, sandbox: bool) -> Self {
        self.sandbox = sandbox;
        self
    }

    /// Subscribe the process to the topics the manifest declares.
    pub fn with_events(self, events: Arc<EventBus>, topics: &[String]) -> Self {
        let mut subscriptions = Vec::new();
        for topic in topics {
            if let Ok(filter) = EventFilter::from_topics(std::slice::from_ref(topic)) {
                match events.subscribe(filter, 64) {
                    Ok(subscription) => subscriptions.push(subscription),
                    Err(error) => log::warn!(
                        target: "bsh::extension",
                        "{}: event subscription failed: {error}",
                        self.extension
                    ),
                }
            }
        }
        if let Ok(mut slot) = self.subscriptions.lock() {
            *slot = subscriptions;
        }
        self
    }

    async fn call_tool_inner(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
        progress: Option<&tokio::sync::mpsc::UnboundedSender<Value>>,
    ) -> Result<Value, ExtensionError> {
        if !self.tools.iter().any(|tool| tool.id.as_str() == name) {
            return Err(ExtensionError::NotFound {
                id: format!("{}::{name}", self.extension),
            });
        }
        let wait = Duration::from_millis(context.timeout_ms.max(1));
        let mut guard = match tokio::time::timeout(wait, self.session.lock()).await {
            Ok(guard) => guard,
            Err(_) => {
                return Err(ExtensionError::Timeout {
                    extension: self.extension.to_string(),
                    operation: format!("{name} (waiting for the process)"),
                    millis: context.timeout_ms,
                });
            }
        };
        if guard.is_none() {
            *guard = Some(self.start_with_retry().await?);
        }
        let session = guard.as_mut().expect("session was just started");
        let result = session
            .request_with_progress(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments,
                    "correlationId": context.correlation_id,
                }),
                context.timeout_ms.max(1).min(self.max_execution_ms),
                &self.extension,
                progress,
            )
            .await;
        match result {
            Ok(value) => {
                let bytes = serde_json::to_vec(&value)
                    .map(|bytes| bytes.len())
                    .unwrap_or(0);
                if bytes as u64 > self.max_output_bytes {
                    *guard = None;
                    return Err(ExtensionError::Limit {
                        extension: self.extension.to_string(),
                        message: format!(
                            "tool result is {bytes} bytes; the ceiling is {}",
                            self.max_output_bytes
                        ),
                    });
                }
                Ok(value)
            }
            Err(error @ ExtensionError::Remote { .. }) => {
                // The server answered; the transport is still in a known state.
                Err(error)
            }
            Err(error) => {
                // A timed-out or broken exchange leaves the pipe in an unknown
                // state, so the next call starts a fresh process instead of
                // reading a response that belongs to the previous call.
                *guard = None;
                Err(error)
            }
        }
    }

    /// The AppContainer this runtime's children run in, created once.
    #[cfg(windows)]
    async fn sandbox(
        &self,
        program: &Path,
    ) -> Result<std::sync::Arc<crate::windows_sandbox::WindowsSandbox>, ExtensionError> {
        let mut slot = self.appcontainer.lock().await;
        if let Some(sandbox) = slot.as_ref() {
            return Ok(std::sync::Arc::clone(sandbox));
        }
        let sandbox = std::sync::Arc::new(
            crate::windows_sandbox::WindowsSandbox::new(
                &self.root,
                self.workspace.as_deref(),
                self.data_root.as_deref(),
                self.memory_bytes,
                program,
            )
            .await
            .map_err(|message| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message,
            })?,
        );
        *slot = Some(std::sync::Arc::clone(&sandbox));
        Ok(sandbox)
    }

    /// Start with two bounded retries; a failed handshake leaves no tool side
    /// effect, so retrying the transport is safe. Delays are 100ms and 250ms.
    async fn start_with_retry(&self) -> Result<ProcessSession, ExtensionError> {
        let mut last = None;
        for delay in [0_u64, 100, 250] {
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            match self.start().await {
                Ok(session) => return Ok(session),
                Err(error) => last = Some(error),
            }
        }
        Err(last.expect("a start attempt ran"))
    }

    async fn start(&self) -> Result<ProcessSession, ExtensionError> {
        let program = if is_allowlisted_command(&self.entry) {
            PathBuf::from(&self.entry)
        } else {
            let relative =
                validate_relative_path(&self.entry).map_err(|reason| ExtensionError::Manifest {
                    path: self.root.join("extension.toml"),
                    message: reason,
                })?;
            let path = self.root.join(relative);
            let canonical_root = self
                .root
                .canonicalize()
                .map_err(|source| ExtensionError::Io {
                    path: self.root.clone(),
                    source,
                })?;
            let canonical = path.canonicalize().map_err(|source| ExtensionError::Io {
                path: path.clone(),
                source,
            })?;
            if !canonical.starts_with(&canonical_root) {
                return Err(ExtensionError::Corrupt {
                    path,
                    message: "process entry escapes the package root".to_string(),
                });
            }
            if !canonical.is_file() {
                return Err(ExtensionError::Manifest {
                    path: canonical,
                    message: "process entry is not a file".to_string(),
                });
            }
            canonical
        };
        if let Some((policy, trust)) = &self.policy {
            policy.authorize_call(
                self.extension.as_str(),
                *trust,
                Capability::ProcessExecute,
                &CallScope {
                    program: Some(self.entry.clone()),
                    args: Some(self.args.clone()),
                    ..CallScope::default()
                },
            )?;
        }
        // bwrap --bind requires its source to exist; the per-extension data
        // directory is created on first write, which may be later than the
        // first process spawn. Create it before building the sandbox plan.
        if let Some(data_root) = &self.data_root {
            std::fs::create_dir_all(data_root).map_err(|source| ExtensionError::Io {
                path: data_root.clone(),
                source,
            })?;
        }
        let token = uuid::Uuid::new_v4().to_string();
        #[cfg(not(windows))]
        let mut child = {
            let mut wrapped = false;
            let mut command = if self.sandbox && cfg!(target_os = "linux") {
                wrapped = true;
                let mut command = Command::new("bwrap");
                command.args(linux_bwrap_plan(
                    &self.root,
                    self.workspace.as_deref(),
                    self.data_root.as_deref(),
                    &program,
                    &self.args,
                ));
                command
            } else if self.sandbox && cfg!(target_os = "macos") {
                let mut command = Command::new("/usr/bin/sandbox-exec");
                command
                    .arg("-p")
                    .arg(sandbox_profile(
                        &self.root,
                        self.workspace.as_deref(),
                        self.data_root.as_deref(),
                    ))
                    .arg(&program);
                command
            } else {
                Command::new(&program)
            };
            configure_process_limits(&mut command, self.memory_bytes, self.cpu_seconds);
            if !wrapped {
                command.args(&self.args);
            }
            command
                .current_dir(&self.root)
                .env_clear()
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            command.env("BSH_IPC_TOKEN", &token);
            for key in ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            for (key, value) in &self.env {
                command.env(key, value);
            }
            command.spawn().map_err(|source| ExtensionError::Io {
                path: program.clone(),
                source,
            })?
        };
        #[cfg(windows)]
        let mut child = {
            if !self.sandbox {
                return Err(ExtensionError::Runtime {
                    extension: self.extension.to_string(),
                    message: "the Windows process runtime requires the AppContainer sandbox"
                        .to_string(),
                });
            }
            let sandbox = self.sandbox(&program).await?;
            let mut envs = vec![("BSH_IPC_TOKEN".to_string(), token.clone())];
            for key in ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL"] {
                if let Some(value) = std::env::var_os(key) {
                    envs.push((key.to_string(), value.to_string_lossy().into_owned()));
                }
            }
            for (key, value) in &self.env {
                envs.push((key.clone(), value.clone()));
            }
            sandbox
                .spawn(&program.to_string_lossy(), &self.args, &self.root, &envs)
                .await
                .map_err(|message| ExtensionError::Runtime {
                    extension: self.extension.to_string(),
                    message,
                })?
        };
        if let Some(pid) = child_id(&child) {
            self.child_pid.store(pid, Ordering::Relaxed);
            #[cfg(unix)]
            self.start_watchdog(pid);
        }
        #[cfg(not(windows))]
        let stdin = child.stdin.take().ok_or_else(|| ExtensionError::Protocol {
            extension: self.extension.to_string(),
            message: "child stdin was not piped".to_string(),
        })?;
        #[cfg(windows)]
        let stdin = tokio::fs::File::from_std(child.take_stdin().ok_or_else(|| {
            ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: "the AppContainer child exposed no stdin".to_string(),
            }
        })?);
        #[cfg(not(windows))]
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: "child stdout was not piped".to_string(),
            })?;
        #[cfg(windows)]
        let stdout = tokio::fs::File::from_std(child.take_stdout().ok_or_else(|| {
            ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: "the AppContainer child exposed no stdout".to_string(),
            }
        })?);
        let stderr_tail = Arc::new(StdMutex::new(VecDeque::<String>::new()));
        #[cfg(not(windows))]
        let stderr = child.stderr.take();
        #[cfg(windows)]
        let stderr = child.take_stderr().map(tokio::fs::File::from_std);
        if let Some(stderr) = stderr {
            let extension = self.extension.clone();
            let tail = Arc::clone(&stderr_tail);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                loop {
                    match read_stderr_line(&mut reader, 64 * 1024).await {
                        Ok(StderrLine::Line(line)) => {
                            let line = String::from_utf8_lossy(&line).trim_end().to_string();
                            log::debug!(
                                target: "bsh::extension",
                                "{} stderr: {}",
                                extension,
                                line
                            );
                            let mut tail = tail.lock().expect("stderr tail lock");
                            if tail.len() == 8 {
                                tail.pop_front();
                            }
                            tail.push_back(line);
                        }
                        Ok(StderrLine::TooLong) => {
                            // An over-long line must not stop the reader: the
                            // child keeps writing, and a discarded pipe turns
                            // every later write into a broken-pipe failure.
                            log::warn!(
                                target: "bsh::extension",
                                "{extension} stderr line exceeded 64 KiB; discarding it"
                            );
                        }
                        Ok(StderrLine::Eof) | Err(_) => break,
                    }
                }
            });
        }
        let mut session = ProcessSession {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            token: token.clone(),
        };
        let initialized = match session
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "extensionId": self.extension.as_str(),
                    "host": { "name": "bsh", "version": HOST_VERSION },
                    "config": self.config,
                    "token": session.token.clone(),
                }),
                HANDSHAKE_MS,
                &self.extension,
            )
            .await
        {
            Ok(initialized) => initialized,
            Err(error) => {
                // The child has exited by the time stdout closes; give the
                // stderr reader a moment to drain before quoting it.
                tokio::time::sleep(Duration::from_millis(50)).await;
                return Err(with_stderr_tail(error, stderr_tail.as_ref()));
            }
        };
        let version = initialized
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !version.starts_with("1.") {
            start_kill(&mut session.child);
            return Err(ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: format!(
                    "process speaks protocol {version:?}; this host requires {PROTOCOL_VERSION}"
                ),
            });
        }
        let echoed = initialized
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if echoed != session.token.as_str() {
            start_kill(&mut session.child);
            return Err(ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: "process did not echo the handshake token".to_string(),
            });
        }
        Ok(session)
    }

    async fn stop(&self) {
        self.stop_watchdog();
        let mut guard = self.session.lock().await;
        let Some(mut session) = guard.take() else {
            return;
        };
        let _ = tokio::time::timeout(
            Duration::from_millis(SHUTDOWN_MS),
            session.request("shutdown", json!({}), SHUTDOWN_MS, &self.extension),
        )
        .await;
        // The child called setpgid(0, 0) in pre_exec, so its pid is the group
        // id. Signal the whole group: a language server's grandchildren would
        // otherwise survive the parent and keep the workspace busy.
        #[cfg(unix)]
        if let Some(pid) = child_id(&session.child) {
            // SAFETY: pid is the group leader this runtime spawned.
            unsafe {
                libc::killpg(pid as libc::pid_t, libc::SIGKILL);
            }
        }
        start_kill(&mut session.child);
        wait_child(&mut session.child).await;
        self.child_pid.store(0, Ordering::Relaxed);
    }
}

#[async_trait]
impl Runtime for ProcessRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Process
    }

    fn memory_limit(&self) -> Option<u64> {
        Some(self.memory_bytes)
    }

    fn memory_used(&self) -> Option<u64> {
        let pid = self.child_pid.load(Ordering::Relaxed);
        (pid > 0).then(|| process_rss_bytes(pid)).flatten()
    }

    async fn activate(&self) -> Result<(), ExtensionError> {
        // On-demand packages still start on their first call, because nothing
        // calls activate for them. An on-startup package is warmed by the host
        // right after load, and this is the one place the child is spawned.
        let mut guard = self.session.lock().await;
        if guard.is_none() {
            *guard = Some(self.start_with_retry().await?);
        }
        Ok(())
    }

    async fn deactivate(&self) -> Result<(), ExtensionError> {
        self.stop().await;
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ExtensionError> {
        self.stop().await;
        Ok(())
    }

    async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        Ok(self.tools.clone())
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        self.call_tool_inner(name, arguments, context, None).await
    }

    async fn call_tool_result(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<ToolResult, ExtensionError> {
        self.call_tool_inner(name, arguments, context, None)
            .await
            .map(ToolResult::from_value)
    }

    async fn command(&self, id: &str, arguments: Value) -> Result<Value, ExtensionError> {
        if !self.commands.iter().any(|command| command.as_str() == id) {
            return Err(ExtensionError::NotFound {
                id: format!("{}::{id}", self.extension),
            });
        }
        let wait = Duration::from_millis(self.max_execution_ms.max(1));
        let mut guard = tokio::time::timeout(wait, self.session.lock())
            .await
            .map_err(|_| ExtensionError::Timeout {
                extension: self.extension.to_string(),
                operation: format!("{id} (waiting for the process)"),
                millis: self.max_execution_ms,
            })?;
        if guard.is_none() {
            *guard = Some(self.start_with_retry().await?);
        }
        let session = guard.as_mut().expect("session was just started");
        let result = session
            .request(
                "commands/execute",
                json!({
                    "name": id,
                    "arguments": arguments,
                }),
                self.max_execution_ms,
                &self.extension,
            )
            .await;
        // A command obeys the same rules as a tool call: a ceiling on the
        // result and a fresh process after a broken exchange. Returning the
        // request directly left a timed-out response on the pipe for the next
        // tools/call to skip.
        match result {
            Ok(value) => {
                let bytes = serde_json::to_vec(&value)
                    .map(|bytes| bytes.len())
                    .unwrap_or(0);
                if bytes as u64 > self.max_output_bytes {
                    *guard = None;
                    return Err(ExtensionError::Limit {
                        extension: self.extension.to_string(),
                        message: format!(
                            "command result is {bytes} bytes; the ceiling is {}",
                            self.max_output_bytes
                        ),
                    });
                }
                Ok(value)
            }
            Err(error @ ExtensionError::Remote { .. }) => Err(error),
            Err(error) => {
                *guard = None;
                Err(error)
            }
        }
    }

    async fn call_tool_progress(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
        progress: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
    ) -> Result<ToolResult, ExtensionError> {
        self.call_tool_inner(name, arguments, context, progress.as_ref())
            .await
            .map(ToolResult::from_value)
    }

    async fn pump(&self) -> Result<(), ExtensionError> {
        let events: Vec<ExtensionEvent> = match self.subscriptions.lock() {
            Ok(subscriptions) => subscriptions
                .iter()
                .filter_map(Subscription::try_recv)
                .collect(),
            Err(_) => Vec::new(),
        };
        if events.is_empty() {
            return Ok(());
        }
        let mut guard = self.session.lock().await;
        let Some(session) = guard.as_mut() else {
            return Ok(());
        };
        for event in events {
            session
                .notify("events/event", json!({ "event": event }), &self.extension)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(unix)]
    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    #[cfg(unix)]
    unsafe extern "C" fn test_setrlimit_ok(
        _resource: RlimitResource,
        _limit: *const libc::rlimit,
    ) -> libc::c_int {
        0
    }

    #[cfg(unix)]
    unsafe extern "C" fn test_setrlimit_fail(
        _resource: RlimitResource,
        _limit: *const libc::rlimit,
    ) -> libc::c_int {
        -1
    }
    #[cfg(unix)]
    unsafe extern "C" fn test_setrlimit_fail_cpu(
        resource: RlimitResource,
        _limit: *const libc::rlimit,
    ) -> libc::c_int {
        if resource == libc::RLIMIT_CPU {
            -1
        } else {
            0
        }
    }

    #[cfg(unix)]
    unsafe extern "C" fn test_setrlimit_fail_nofile(
        resource: RlimitResource,
        _limit: *const libc::rlimit,
    ) -> libc::c_int {
        if resource == libc::RLIMIT_NOFILE {
            -1
        } else {
            0
        }
    }

    #[cfg(unix)]
    unsafe extern "C" fn test_setpgid_ok(_pid: libc::pid_t, _pgid: libc::pid_t) -> libc::c_int {
        0
    }

    #[cfg(unix)]
    unsafe extern "C" fn test_setpgid_fail(_pid: libc::pid_t, _pgid: libc::pid_t) -> libc::c_int {
        -1
    }

    #[test]
    fn resident_memory_uses_statm_field_two_and_the_system_page_size() {
        let statm = "100 25 10 1 0 20 0";
        assert_eq!(statm_rss_bytes(statm, 4096), Some(25 * 4096));
        assert_eq!(statm_rss_bytes(statm, 16384), Some(25 * 16384));
        assert_ne!(
            statm_rss_bytes(statm, 4096),
            Some(100 * 4096),
            "field 1 is virtual size, not resident memory"
        );
        assert_eq!(statm_rss_bytes("", 4096), None);
        assert_eq!(statm_rss_bytes("not a statm line", 4096), None);
    }

    #[test]
    fn process_start_time_is_read_after_the_comm_field() {
        let stat =
            "123 (a name with spaces) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987654 22 33";
        assert_eq!(stat_start_time(stat), Some(987654));
        assert_eq!(stat_start_time("no parentheses at all"), None);
        assert_eq!(stat_start_time("1 (short) S 1 2"), None);
    }

    #[tokio::test]
    async fn an_oversized_stderr_line_is_discarded_and_the_next_line_is_read() {
        let data = format!("{}tail\nnext\n", "x".repeat(64));
        let mut reader = BufReader::new(data.as_bytes());
        assert!(matches!(
            read_stderr_line(&mut reader, 16).await,
            Ok(StderrLine::TooLong)
        ));
        match read_stderr_line(&mut reader, 64).await {
            Ok(StderrLine::Line(line)) => assert_eq!(line, b"next\n"),
            _ => panic!("the next stderr line must still be readable"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_job_kills_its_child_when_the_handle_closes() {
        use std::os::windows::io::AsRawHandle as _;
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "ping", "-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("child");
        let job = crate::windows_job::JobObject::new(64 * 1024 * 1024).expect("job object");
        job.assign(child.as_raw_handle() as _).expect("assign");
        drop(job);
        for _ in 0..50 {
            if child.try_wait().expect("wait").is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        panic!("closing the job did not kill its child");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_memory_watchdog_kills_a_child_over_its_ceiling() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let mut child = Command::new("python3")
            .args([
                "-c",
                "x = b'x' * (256 * 1024 * 1024)\nimport time\ntime.sleep(30)",
            ])
            .spawn()
            .expect("memory hog");
        let pid = child.id().expect("pid");
        let identity = child_identity(pid);
        assert!(identity.is_some(), "the watchdog must identify its child");
        let watchdog = spawn_memory_watchdog(pid, 32 * 1024 * 1024, identity);
        let mut exited = false;
        for _ in 0..50 {
            if child.try_wait().expect("wait").is_some() {
                exited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        watchdog.stop();
        let _ = child.start_kill();
        let _ = child.wait().await;
        assert!(
            exited,
            "a child that exceeds its memory ceiling must be terminated"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_live_process_has_a_stable_start_time_identity() {
        let pid = std::process::id();
        let identity = child_identity(pid);
        assert!(
            identity.is_some(),
            "the watchdog must be able to identify a live child"
        );
        assert_eq!(identity, child_identity(pid));
        assert!(identity_matches(pid, identity.as_deref()));
        assert!(
            !identity_matches(pid, Some("not-this-process")),
            "a stale identity must not authorize a signal"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_resource_limit_setup_fails_closed() {
        let limit_failure = unsafe {
            apply_unix_limits(64 * 1024 * 1024, 10, test_setrlimit_fail, test_setpgid_ok)
        };
        assert!(limit_failure.is_err(), "failed setrlimit must abort spawn");

        let cpu_failure = unsafe {
            apply_unix_limits(
                64 * 1024 * 1024,
                10,
                test_setrlimit_fail_cpu,
                test_setpgid_ok,
            )
        };
        assert!(cpu_failure.is_err(), "failed RLIMIT_CPU must abort spawn");

        let nofile_failure = unsafe {
            apply_unix_limits(
                64 * 1024 * 1024,
                10,
                test_setrlimit_fail_nofile,
                test_setpgid_ok,
            )
        };
        assert!(
            nofile_failure.is_err(),
            "failed RLIMIT_NOFILE must abort spawn"
        );

        let process_group_failure = unsafe {
            apply_unix_limits(64 * 1024 * 1024, 10, test_setrlimit_ok, test_setpgid_fail)
        };
        assert!(
            process_group_failure.is_err(),
            "failed process-group setup must abort spawn"
        );

        let success =
            unsafe { apply_unix_limits(64 * 1024 * 1024, 10, test_setrlimit_ok, test_setpgid_ok) };
        assert!(success.is_ok());
    }

    #[test]
    fn the_macos_profile_denies_writes_outside_declared_roots() {
        let profile = sandbox_profile(Path::new("/pkg"), None, None);
        assert!(profile.contains("(deny file-write*)"));
        assert!(profile.contains("(deny network*)"));
        assert!(profile.contains("(subpath \"/pkg\")"));
        assert!(profile.contains("(subpath \"/tmp\")"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_macos_sandbox_denies_network_access() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("address").port();
        let profile = sandbox_profile(Path::new("/tmp"), None, None);
        let script = format!(
            "import socket; s=socket.socket(); s.settimeout(1); s.connect(('127.0.0.1',{port}))"
        );
        let output = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(&profile)
            .arg("python3")
            .arg("-c")
            .arg(script)
            .output()
            .expect("sandbox-exec");
        assert!(
            !output.status.success(),
            "the child reached the network inside the sandbox: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_linux_plan_recreates_only_declared_roots_under_hidden_home() {
        let plan = linux_bwrap_plan(
            Path::new("/home/alice/projects/plugin"),
            Some(Path::new("/home/alice/workspace")),
            None,
            Path::new("/home/alice/projects/plugin/server"),
            &[],
        );
        let hidden_home = plan
            .iter()
            .position(|item| item == "/home")
            .expect("home is hidden");
        let home_plugin_dir = plan
            .iter()
            .position(|item| item == "/home/alice/projects/plugin")
            .expect("declared package mount point is recreated");
        let package_bind = plan
            .iter()
            .enumerate()
            .skip(home_plugin_dir)
            .find(|(_, item)| *item == "--bind")
            .map(|(index, _)| index)
            .expect("package bind");
        assert!(hidden_home < home_plugin_dir);
        assert!(home_plugin_dir < package_bind);
        assert!(!plan.iter().any(|item| item == "/home/alice/.ssh"));
    }

    #[test]
    fn the_linux_plan_creates_tmpfs_targets_before_binding_temp_roots() {
        let plan = linux_bwrap_plan(
            Path::new("/tmp/bsh-package"),
            Some(Path::new("/tmp/bsh-package/workspace")),
            None,
            Path::new("/tmp/bsh-package/server"),
            &[],
        );
        let dir = plan
            .iter()
            .position(|item| item == "--dir")
            .expect("tmpfs target dir");
        let bind = plan
            .iter()
            .position(|item| item == "--bind")
            .expect("package bind");
        assert!(dir < bind, "the package destination must exist before bind");
        // The package root is bound writable; the nested workspace is covered
        // by that bind, so one bind is the correct plan.
        assert_eq!(
            plan.iter().filter(|item| item.as_str() == "--bind").count(),
            1
        );
    }

    #[test]
    fn the_linux_plan_binds_only_writable_roots_and_appends_the_command() {
        let plan = linux_bwrap_plan(
            Path::new("/pkg"),
            Some(Path::new("/work")),
            Some(Path::new("/data")),
            Path::new("/pkg/server"),
            &["--flag".to_string()],
        );
        assert!(plan.contains(&"--unshare-all".to_string()));
        assert!(
            !plan.contains(&"--share-net".to_string()),
            "the sandbox must not rejoin the host network namespace"
        );
        for hidden in ["/home", "/root", "/run"] {
            let index = plan
                .iter()
                .position(|item| item == hidden)
                .expect("hidden root");
            assert_eq!(plan[index - 1], "--tmpfs");
        }
        assert_eq!(
            plan.iter().filter(|item| item.as_str() == "--bind").count(),
            3
        );
        assert_eq!(plan[plan.len() - 2], "/pkg/server");
        assert_eq!(plan[plan.len() - 1], "--flag");
    }

    #[cfg(unix)]
    struct Scratch(PathBuf);

    #[cfg(unix)]
    impl Scratch {
        fn new(label: &str) -> Self {
            let index = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            // Linux replaces /tmp with a private tmpfs in the process sandbox.
            // Keep the package root elsewhere so the explicit writable bind remains visible.
            #[cfg(target_os = "linux")]
            let base = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            #[cfg(not(target_os = "linux"))]
            let base = std::env::temp_dir();
            let path = base.join(format!(
                "bsh-ext-process-{label}-{}-{index}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join("tools")).expect("scratch");
            Self(path)
        }
    }

    #[cfg(unix)]
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn manifest() -> Manifest {
        Manifest::parse(
            r#"
schema_version = "1.0"

[extension]
id = "com.example.process"
name = "Process"
version = "1.0.0"
description = "A process package"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "process"
entry = "sh"
args = ["server.sh"]

[capabilities]
required = ["process.execute"]

[contributes]
tools = ["echo"]

[resources]
max_execution_ms = 2000
"#,
            Path::new("extension.toml"),
        )
        .expect("manifest")
    }

    #[test]
    fn deeply_nested_json_is_refused() {
        let mut value = json!(0);
        for _ in 0..40 {
            value = json!([value]);
        }
        assert!(json_depth_ok(&value, 0).is_err());
        assert!(json_depth_ok(&json!({"ok": [1, 2, 3]}), 0).is_ok());
    }

    #[tokio::test]
    async fn read_line_is_bounded_and_line_oriented() {
        let mut reader = BufReader::new(&b"a\nbb\ncc"[..]);
        let id = ExtensionId::parse("com.example.a").expect("id");
        assert_eq!(
            read_limited_line(&mut reader, 16, &id).await.expect("line"),
            Some(b"a\n".to_vec())
        );
        assert_eq!(
            read_limited_line(&mut reader, 16, &id).await.expect("line"),
            Some(b"bb\n".to_vec())
        );
        assert_eq!(
            read_limited_line(&mut reader, 16, &id).await.expect("line"),
            Some(b"cc".to_vec())
        );
        assert!(read_limited_line(&mut reader, 16, &id)
            .await
            .expect("eof")
            .is_none());
        let mut short = BufReader::new(&b"aaaaaaaaaaaaaaaaaaaa\n"[..]);
        assert!(read_limited_line(&mut short, 8, &id).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_declared_event_is_forwarded_to_the_process() {
        let scratch = Scratch::new("events");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        std::fs::write(
            scratch.0.join("server.sh"),
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0","token":"%s"}}\n' "$BSH_IPC_TOKEN" ;;
    *'tools/call'*) echo '{"jsonrpc":"2.0","id":2,"result":{"ok":true}}' ;;
    *'events/event'*) echo "$line" >> events.log ;;
  esac
done
"#,
        )
        .expect("server");
        let manifest = Manifest::parse(
            r#"
schema_version = "1.0"

[extension]
id = "com.example.events"
name = "Events"
version = "1.0.0"
description = "A process event subscriber"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "process"
entry = "sh"
args = ["server.sh"]

[capabilities]
required = ["process.execute"]

[contributes]
tools = ["echo"]
events = ["workspace.changed"]
"#,
            Path::new("extension.toml"),
        )
        .expect("manifest");
        let bus = EventBus::new();
        let runtime = ProcessRuntime::new(&manifest, &scratch.0)
            .expect("runtime")
            .with_events(Arc::clone(&bus), &manifest.contributes.events);
        runtime.activate().await.expect("activate");
        let context = CallContext::new(manifest.id().clone(), "events", None, 2000);
        runtime
            .call_tool("echo", json!({}), &context)
            .await
            .expect("first call starts the process");
        let event = ExtensionEvent::new("bsh", "workspace.changed", json!({"root": "/tmp"}))
            .expect("event");
        bus.publish(&event).expect("publish");
        runtime.pump().await.expect("pump");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let logged = std::fs::read_to_string(scratch.0.join("events.log")).expect("events log");
        assert!(logged.contains("workspace.changed"), "{logged}");
        runtime.shutdown().await.expect("shutdown");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_linux_sandbox_denies_a_write_outside_the_package() {
        let scratch = Scratch::new("linux-sandbox");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .expect("CI runner HOME is set");
        let probe = home.join(format!(
            "bsh-ext-linux-sandbox-probe-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&probe);
        std::fs::write(
            scratch.0.join("server.sh"),
            r#"while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0","token":"%s"}}\n' "$BSH_IPC_TOKEN" ;;
    *'"tools/call"'*)
      if echo x > "$HOME/bsh-ext-linux-sandbox-probe-PID" 2>/dev/null; then
        echo '{"jsonrpc":"2.0","id":2,"result":{"blocked":false}}'
      else
        echo '{"jsonrpc":"2.0","id":2,"result":{"blocked":true}}'
      fi
      ;;
  esac
done
"#
            .replace("PID", &std::process::id().to_string()),
        )
        .expect("server");
        let runtime = ProcessRuntime::new(&manifest(), &scratch.0).expect("runtime");
        assert!(
            runtime.sandbox,
            "Linux process sandbox must be enabled by default"
        );
        let context = CallContext::new(manifest().id().clone(), "linux-sandbox", None, 2000);
        let result = runtime
            .call_tool("echo", json!({}), &context)
            .await
            .expect("sandboxed call");
        assert_eq!(
            result["blocked"], true,
            "HOME is outside writable sandbox mounts"
        );
        assert!(
            !probe.exists(),
            "the extension must not write into the host HOME"
        );
        runtime.shutdown().await.expect("shutdown");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn the_macos_sandbox_denies_a_write_outside_the_package() {
        let scratch = Scratch::new("sandbox");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        let probe = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join(format!("bsh-ext-sandbox-probe-{}", std::process::id()));
        let _ = std::fs::remove_file(&probe);
        std::fs::write(
            scratch.0.join("server.sh"),
            r#"while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0","token":"%s"}}\n' "$BSH_IPC_TOKEN" ;;
    *'"tools/call"'*)
      if echo x > "$HOME/bsh-ext-sandbox-probe-PID" 2>/dev/null; then
        echo '{"jsonrpc":"2.0","id":2,"result":{"blocked":false}}'
      else
        echo '{"jsonrpc":"2.0","id":2,"result":{"blocked":true}}'
      fi
      ;;
  esac
done
"#
            .replace("PID", &std::process::id().to_string()),
        )
        .expect("server");
        let runtime = ProcessRuntime::new(&manifest(), &scratch.0)
            .expect("runtime")
            .with_sandbox(true);
        let context = CallContext::new(manifest().id().clone(), "sandbox", None, 2000);
        let result = runtime
            .call_tool("echo", json!({}), &context)
            .await
            .expect("call");
        assert_eq!(result["blocked"], true);
        assert!(!probe.exists());
        runtime.shutdown().await.expect("shutdown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_process_call_times_out_at_its_manifest_ceiling() {
        let scratch = Scratch::new("timeout");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        std::fs::write(
            scratch.0.join("server.py"),
            r#"import json, sys, time
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        result = {"protocolVersion": "1.0", "token": request.get("params", {}).get("token")}
    elif method == "tools/call":
        time.sleep(10)
        result = {"ok": True}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request.get("id"), "result": result}), flush=True)
"#,
        )
        .expect("server");
        let manifest = Manifest::parse(
            r#"
schema_version = "1.0"
[extension]
id = "com.example.process"
name = "Process"
version = "1.0.0"
description = "A process package"
[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"
[runtime]
kind = "process"
entry = "python3"
args = ["server.py"]
[capabilities]
required = ["process.execute"]
[contributes]
tools = ["echo"]
[resources]
max_execution_ms = 500
"#,
            &scratch.0.join("extension.toml"),
        )
        .expect("manifest");
        let runtime = ProcessRuntime::new(&manifest, &scratch.0).expect("runtime");
        let context = CallContext::new(manifest.id().clone(), "timeout", None, 5000);
        let error = runtime
            .call_tool("echo", json!({}), &context)
            .await
            .expect_err("the manifest ceiling wins");
        assert_eq!(error.category(), "timeout");
        runtime.shutdown().await.expect("shutdown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_process_memory_ceiling_kills_a_runaway_child() {
        let scratch = Scratch::new("memory-cap");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        std::fs::write(
            scratch.0.join("server.py"),
            r#"import json, sys, time
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        result = {"protocolVersion": "1.0", "token": request.get("params", {}).get("token")}
    elif method == "tools/call":
        chunks = []
        for _ in range(40):
            chunks.append(bytearray(32 * 1024 * 1024))
            time.sleep(0.05)
        result = {"limited": False}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request.get("id"), "result": result}), flush=True)
"#,
        )
        .expect("server");
        let manifest = Manifest::parse(
            r#"
schema_version = "1.0"
[extension]
id = "com.example.process"
name = "Process"
version = "1.0.0"
description = "A process package"
[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"
[runtime]
kind = "process"
entry = "python3"
args = ["server.py"]
[capabilities]
required = ["process.execute"]
[contributes]
tools = ["echo"]
[resources]
max_memory_mb = 128
max_execution_ms = 5000
"#,
            &scratch.0.join("extension.toml"),
        )
        .expect("manifest");
        let runtime = ProcessRuntime::new(&manifest, &scratch.0).expect("runtime");
        let context = CallContext::new(manifest.id().clone(), "memory-cap", None, 5000);
        let error = runtime
            .call_tool("echo", json!({}), &context)
            .await
            .expect_err("the watchdog kills the child");
        assert!(matches!(error.category(), "runtime" | "timeout"), "{error}");
        runtime.shutdown().await.expect("shutdown");
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_calls_share_one_child_without_exhaustion() {
        let scratch = Scratch::new("concurrent");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        std::fs::write(
            scratch.0.join("server.py"),
            r#"import json, sys
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        result = {"protocolVersion": "1.0", "token": request.get("params", {}).get("token")}
    elif method == "tools/call":
        result = {"ok": True}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request.get("id"), "result": result}), flush=True)
"#,
        )
        .expect("server");
        let manifest = Manifest::parse(
            r#"
schema_version = "1.0"
[extension]
id = "com.example.process"
name = "Process"
version = "1.0.0"
description = "A process package"
[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"
[runtime]
kind = "process"
entry = "python3"
args = ["server.py"]
[capabilities]
required = ["process.execute"]
[contributes]
tools = ["echo"]
[resources]
max_execution_ms = 2000
"#,
            &scratch.0.join("extension.toml"),
        )
        .expect("manifest");
        let runtime =
            std::sync::Arc::new(ProcessRuntime::new(&manifest, &scratch.0).expect("runtime"));
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let runtime = std::sync::Arc::clone(&runtime);
            let id = manifest.id().clone();
            tasks.push(tokio::spawn(async move {
                let context = CallContext::new(id, "concurrent", None, 3000);
                runtime.call_tool("echo", json!({}), &context).await
            }));
        }
        for task in tasks {
            let result = task.await.expect("join").expect("call");
            assert_eq!(result["ok"], true);
        }
        runtime.shutdown().await.expect("shutdown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_missing_handshake_token_is_refused() {
        let scratch = Scratch::new("token");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        std::fs::write(
            scratch.0.join("server.sh"),
            r#"while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*) echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0"}}' ;;
    *'"tools/call"'*) echo '{"jsonrpc":"2.0","id":2,"result":{"ok":true}}' ;;
  esac
done
"#,
        )
        .expect("server");
        let runtime = ProcessRuntime::new(&manifest(), &scratch.0).expect("runtime");
        let context = CallContext::new(manifest().id().clone(), "token", None, 2000);
        let error = runtime
            .call_tool("echo", json!({}), &context)
            .await
            .expect_err("token");
        assert!(error.to_string().contains("token"), "{error}");
        runtime.shutdown().await.expect("shutdown");
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn a_process_answers_the_handshake_and_a_tool_call() {
        let scratch = Scratch::new("call");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        std::fs::write(
            scratch.0.join("server.sh"),
            r#"while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0","token":"%s"}}\n' "$BSH_IPC_TOKEN" ;;
    *'"tools/call"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"ok":true}}' ;;
    *) : ;;
  esac
done
"#,
        )
        .expect("server");
        let runtime = ProcessRuntime::new(&manifest(), &scratch.0).expect("runtime");
        let context = CallContext::new(manifest().id().clone(), "correlation", None, 2000);
        let result = runtime
            .call_tool("echo", serde_json::json!({"x": 1}), &context)
            .await
            .expect("call");
        assert_eq!(result["ok"], true);
        assert!(
            runtime.memory_used().unwrap_or(0) > 0,
            "the live child reports its resident memory"
        );
        runtime.shutdown().await.expect("shutdown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_tool_call_forwards_progress_notifications() {
        let scratch = Scratch::new("progress");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        std::fs::write(
            scratch.0.join("server.sh"),
            r#"while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0","token":"%s"}}\n' "$BSH_IPC_TOKEN" ;;
    *'"tools/call"'*)
      echo '{"jsonrpc":"2.0","method":"$/progress","params":{"message":"half"}}'
      echo '{"jsonrpc":"2.0","id":2,"result":{"ok":true}}'
      ;;
    *) : ;;
  esac
done
"#,
        )
        .expect("server");
        let runtime = ProcessRuntime::new(&manifest(), &scratch.0).expect("runtime");
        let context = CallContext::new(manifest().id().clone(), "progress", None, 2000);
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let result = runtime
            .call_tool_progress("echo", serde_json::json!({"x": 1}), &context, Some(sender))
            .await
            .expect("call");
        assert_eq!(result.to_value()["ok"], true);
        let progress = receiver.recv().await.expect("one progress");
        assert_eq!(progress["message"], "half");
        runtime.shutdown().await.expect("shutdown");
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn a_missing_entry_is_an_error_not_a_panic() {
        let scratch = Scratch::new("missing");
        std::fs::write(scratch.0.join("tools/echo.json"), "{}").expect("tool");
        let mut manifest = manifest();
        manifest.runtime.entry = Some("missing-binary".to_string());
        manifest.runtime.args.clear();
        let runtime = ProcessRuntime::new(&manifest, &scratch.0).expect("runtime");
        let context = CallContext::new(manifest.id().clone(), "correlation", None, 200);
        let error = runtime
            .call_tool("echo", serde_json::json!({}), &context)
            .await
            .expect_err("missing entry");
        assert!(matches!(error, ExtensionError::Io { .. }), "{error}");
    }
}
