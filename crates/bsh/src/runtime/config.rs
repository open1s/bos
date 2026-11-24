//! Host configuration: where durable state lives and how the executor is built.
//!
//! Paths come from the command line and the environment; everything else
//! comes from `host.toml` beside the journal, so a Host is reproducible from
//! its data directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::runtime::executor::ReactSettings;

/// Default number of committed events retained for cursor replay.
pub const DEFAULT_REPLAY_WINDOW: u64 = 4096;

/// Default per-subscription live buffer.
pub const DEFAULT_SUBSCRIPTION_BUFFER: usize = 256;

/// Default maximum size of one IPC frame.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Default number of simultaneous client connections.
pub const DEFAULT_MAX_CONNECTIONS: usize = 64;

/// Default request budget per connection, per second.
pub const DEFAULT_REQUESTS_PER_SECOND: u32 = 200;

/// Default graceful-shutdown in-flight budget.
pub const DEFAULT_SHUTDOWN_GRACE_SECS: u64 = 5;

/// Everything a [`super::host::RuntimeHost`] needs to start.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Directory holding the journal, audit log, lock and endpoint record.
    pub data_dir: PathBuf,
    /// Directory the agent works in. Its canonical path derives the
    /// `WorkspaceId`, so the identity is stable across restarts.
    pub workspace_root: PathBuf,
    /// Local IPC socket path. Defaults to `<data_dir>/host.sock`.
    pub socket_path: PathBuf,
    /// Committed events retained for replay.
    pub replay_window: u64,
    /// Per-subscription live buffer.
    pub subscription_buffer: usize,
    /// Maximum accepted frame size in bytes.
    pub max_frame_bytes: usize,
    /// Maximum simultaneous connections.
    pub max_connections: usize,
    /// Per-connection request budget per second.
    pub requests_per_second: u32,
    /// How long a graceful shutdown lets in-flight turns finish.
    pub shutdown_grace: Duration,
    /// Use the deterministic echo executor instead of the model-backed one.
    pub echo: bool,
    /// Provider settings for the model-backed executor.
    pub provider: ReactSettings,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        let data_dir = PathBuf::from(".bos/bsh-runtime");
        Self {
            socket_path: data_dir.join("host.sock"),
            data_dir,
            workspace_root: PathBuf::from("."),
            replay_window: DEFAULT_REPLAY_WINDOW,
            subscription_buffer: DEFAULT_SUBSCRIPTION_BUFFER,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            requests_per_second: DEFAULT_REQUESTS_PER_SECOND,
            shutdown_grace: Duration::from_secs(DEFAULT_SHUTDOWN_GRACE_SECS),
            echo: false,
            provider: ReactSettings::default(),
        }
    }
}

/// The on-disk `host.toml` surface.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    runtime: FileRuntime,
    agent: FileAgent,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileRuntime {
    replay_window: u64,
    subscription_buffer: usize,
    max_frame_bytes: usize,
    max_connections: usize,
    requests_per_second: u32,
    shutdown_grace_secs: u64,
}

impl Default for FileRuntime {
    fn default() -> Self {
        Self {
            replay_window: DEFAULT_REPLAY_WINDOW,
            subscription_buffer: DEFAULT_SUBSCRIPTION_BUFFER,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            requests_per_second: DEFAULT_REQUESTS_PER_SECOND,
            shutdown_grace_secs: DEFAULT_SHUTDOWN_GRACE_SECS,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileAgent {
    model: String,
    base_url: String,
    api_key: String,
    system_prompt: String,
    max_steps: usize,
}

impl RuntimeConfig {
    /// A configuration rooted at `data_dir` with the default file settings.
    #[must_use]
    pub fn with_data_dir(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        Self {
            socket_path: data_dir.join("host.sock"),
            data_dir,
            ..Self::default()
        }
    }

    /// Load `<data_dir>/host.toml` when present and apply it.
    ///
    /// A missing file is not an error: a fresh Host starts with defaults and
    /// the echo executor is refused unless explicitly requested.
    pub fn load(mut self) -> Result<Self, String> {
        let path = self.data_dir.join("host.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(self),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let file: FileConfig =
            toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        self.replay_window = file.runtime.replay_window.max(1);
        self.subscription_buffer = file.runtime.subscription_buffer.max(1);
        self.max_frame_bytes = file.runtime.max_frame_bytes.max(1024);
        self.max_connections = file.runtime.max_connections.max(1);
        self.requests_per_second = file.runtime.requests_per_second.max(1);
        self.shutdown_grace = Duration::from_secs(file.runtime.shutdown_grace_secs);
        self.provider = ReactSettings {
            model: file.agent.model,
            base_url: file.agent.base_url,
            api_key: file.agent.api_key,
            system_prompt: file.agent.system_prompt,
            max_steps: file.agent.max_steps,
        };
        Ok(self)
    }

    /// Path of the durable event journal.
    #[must_use]
    pub fn journal_path(&self) -> PathBuf {
        self.data_dir.join("journal.jsonl")
    }

    /// Path of the security audit log.
    #[must_use]
    pub fn audit_path(&self) -> PathBuf {
        self.data_dir.join("audit.jsonl")
    }

    /// Path of the single-instance lock file.
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        // `hostkit::coord::FileLock` appends `.lock`, so the identity path is
        // `host` and the file on disk is `host.lock`.
        self.data_dir.join("host")
    }

    /// Path of the discovery record clients read.
    #[must_use]
    pub fn endpoint_path(&self) -> PathBuf {
        self.data_dir.join("runtime.json")
    }

    /// Path of an existing `host.toml`, for diagnostics.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.data_dir.join("host.toml")
    }

    /// Whether the configured socket path is inside the data directory.
    #[must_use]
    pub fn socket_is_private(&self) -> bool {
        self.socket_path.starts_with(&self.data_dir)
    }

    /// Normalize the workspace identity: the canonical directory path.
    #[must_use]
    pub fn canonical_workspace(&self) -> PathBuf {
        std::fs::canonicalize(&self.workspace_root).unwrap_or_else(|_| self.workspace_root.clone())
    }

    /// The data directory, for diagnostics.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}
