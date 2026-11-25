//! The Host's desktop-parity executor.
//!
//! A `bsh tui` or `bsh host` turn runs through the very agent `caps` builds for
//! a desktop turn - the same files, shell, skills, subagents, jobs, browser and
//! extension surface, resolved from the same `Settings` and `Vault` - instead
//! of the tool-less Stage-1 ReAct loop. The Host keeps ownership of the turn
//! lifecycle; this module owns the model/tool work of exactly one accepted turn
//! and never touches session state directly.
//!
//! What a turn still lacks, and why:
//!
//! - **Per-call approval.** `caps` bakes the run's policy into the build, and
//!   the only approval gate the Host offers an executor arrives per turn. A
//!   prompt per tool call needs its own contract fact so a client can show the
//!   action, so the gate is asked once, and only for a turn whose permission is
//!   unconfined (`danger-full-access`): that is the case where the model's
//!   commands leave the workspace boundary entirely.
//! - **MCP servers.** The pool is reconciled from the same settings a window
//!   reads when the Host opens, so a terminal turn calls the tools a desktop
//!   turn calls. A server that fails to connect is left out of the pool and
//!   reported through the catalogue rather than failing the Host.
//! - **Goals and review snapshots.** No client renders either yet.
//!
//! History is seeded into the engine's own session before the turn, the way the
//! desktop runner seeds a resumed chat: a second turn in one session reads the
//! first one, and every turn after it reads the whole transcript.
//!
//! A turn resolves the machine's default model and default preset. The Host
//! records neither per session yet, so a session cannot yet carry its own
//! model the way a desktop chat does.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use react::llm::{Content, StreamToken};

use super::contract::{
    ApprovalRequest, ApprovalTarget, BrowserElementView, BrowserView, CatalogueView, ExtensionView,
    ExtensionsReport, FederationGrantView, FederationPeerView, FederationReport, McpServerView,
    MemoryReport, MessageRole, ProviderModels, ReviewReport, SessionConfig, SessionConfigChange,
    SettingEdit, SettingsReport, SettingsRowView, SettingsSectionView, SkillView, SubagentReport,
    SubagentRunView, TaskAttemptView, TaskBoardReport, TaskDetailReport, TaskRowView,
    TaskSectionView, TurnUsage, WorkspaceChangeView, WorkspaceDiffView, WorkspaceEntryView,
    WorkspaceReport,
};
use super::executor::{
    CompactContext, CompactionOutput, TurnContext, TurnError, TurnExecutor, TurnOutput,
    TurnProgress, apply_config_change,
};
use crate::i18n::Lang;
use crate::settings::Settings;
use crate::settings::global::ModelRef;
use crate::settings::resolve::SessionChoice;
use crate::vault::Vault;

/// How many characters of a tool call's arguments travel per fact.
///
/// The transcript shows the call; the model saw the whole argument document.
/// A cap keeps a large patch from turning a journal append into a frame the
/// transport has to refuse.
const ARGS_CAP: usize = 4_000;

/// Tokens the manual `/compact` keeps verbatim.
///
/// Zero is what the desktop command uses: `/compact` does not wait for
/// context pressure, so it folds every span the agent considers safe instead
/// of leaving a tail behind.
const COMPACT_RETAIN: usize = 0;

/// How much tool output travels per fact.
const OUTPUT_CAP: usize = 8_000;

/// How much of an argument leads the one-line summary.
const SUMMARY_CAP: usize = 120;

/// How much assistant text one activity frame carries.
///
/// Providers emit deltas at whatever granularity they stream; forwarding each
/// one would turn a paragraph into hundreds of frames a consumer cannot use
/// faster than it reads them. Coalescing keeps a live answer smooth without
/// hiding that it is still arriving.
const DELTA_CHARS: usize = 80;

/// How long an operator has to answer a turn-level approval.
const TURN_APPROVAL_SECS: u64 = 600;

/// How long a Host waits for its MCP roster before it starts serving.
///
/// Connecting is a process spawn or an HTTP handshake, and the desktop pays the
/// same cost when it launches. A server that has not answered by then is absent
/// from the pool; the turn runs without its tools and the roster says so.
const MCP_CONNECT_SECS: u64 = 10;

/// The desktop agent behind a Runtime Host turn.
pub(crate) struct AgentExecutor {
    /// The shell's settings, loaded when the Host opened.
    ///
    /// A write replaces the snapshot whole rather than editing it in place, so
    /// a turn that cloned it holds the configuration of one moment and an edit
    /// reaches the next turn instead of reshaping one in flight. The lock is a
    /// plain `RwLock` because every read is short and none crosses an `await`.
    settings: std::sync::RwLock<Settings>,
    /// The shell's credential store; the only source of a provider key.
    vault: Vault,
    /// The MCP servers this build connected at open, shared by every turn.
    mcp: crate::mcp::McpPool,
    /// Live background-job roster, shared by every turn of this Host: a job a
    /// previous turn started stays readable and killable by the next one.
    jobs: Arc<agent::jobs::JobRegistry>,
    /// Child-agent registry, shared the same way.
    subagents: Arc<crate::subagent::SubagentRegistry>,
    /// The browser handle a turn's browser tools drive.
    browser: Arc<crate::browser::BrowserHandle>,
    /// The settings workspace record the Host's root resolves to.
    ///
    /// A session that names no workspace falls back to the *active* one, which
    /// would point the file tools at whichever project the desktop last
    /// opened. Naming the Host's own record keeps the tools inside the
    /// directory this Host serves.
    workspace: String,
    /// Language for the messages a client renders.
    lang: Lang,
}

impl AgentExecutor {
    /// Build the executor the way `bsh` itself reads its configuration.
    ///
    /// `workspace_root` is the directory the Host serves. It becomes (or
    /// matches) a workspace record in the loaded settings, so the build's
    /// effective root - the tree the file tools may touch - is the directory
    /// the reader started the Host in.
    pub(crate) fn open(workspace_root: &Path) -> Self {
        let mut settings = Settings::load();
        let root = canonical_root(workspace_root);
        // The workspace document follows the directory the Host serves, the
        // way `bsh --workspace <dir>` follows it: the permission, preset and
        // sketch a turn runs under are the ones written for this project.
        if let Ok(workspace) = crate::settings::workspace::WorkspaceSettings::load(
            &crate::settings::workspace::WorkspaceSettings::path(&root),
        ) {
            settings.workspace = workspace;
        }
        // Reuse the reader's record when one names this root, so a project
        // already opened in `bsh` keeps its preset, sketch and instructions.
        let existing = settings
            .global
            .workspaces
            .iter()
            .find(|workspace| same_root(&workspace.root, &root))
            .map(|workspace| workspace.id.clone());
        let workspace = existing.unwrap_or_else(|| {
            let name = root
                .file_name()
                .map(|leaf| leaf.to_string_lossy().into_owned())
                .filter(|leaf| !leaf.is_empty())
                .unwrap_or_else(|| root.to_string_lossy().into_owned());
            settings.note_workspace(&name, &root.to_string_lossy())
        });
        // The registry the desktop mutates lives on disk; the Host's copy is
        // in memory only, so starting a Host in a fresh directory never
        // rewrites the reader's settings file.
        let vault = Vault::new(settings.vault_path());
        let lang = Lang::from_locale(&settings.global.general.lang);
        // The pool is the desktop's: the servers the settings name, plus the
        // ones installed extensions declare and the trust gate releases. It is
        // connected once, here, because a connection is a child process and
        // every turn of this Host shares it.
        let choice = SessionChoice {
            workspace: Some(workspace.clone()),
            ..SessionChoice::default()
        };
        let (mcp, failed) = connect_mcp(&settings, &choice);
        if !failed.is_empty() {
            log::warn!(
                "mcp servers unavailable for this Host: {}",
                failed.join(", ")
            );
        }
        Self::with_parts(settings, vault, workspace, lang, mcp)
    }

    /// Assemble an executor from parts that are already loaded.
    fn with_parts(
        settings: Settings,
        vault: Vault,
        workspace: String,
        lang: Lang,
        mcp: crate::mcp::McpPool,
    ) -> Self {
        Self {
            settings: std::sync::RwLock::new(settings),
            vault,
            mcp,
            jobs: Arc::new(agent::jobs::JobRegistry::new()),
            subagents: Arc::new(crate::subagent::SubagentRegistry::default()),
            browser: Arc::new(crate::browser::BrowserHandle::new()),
            workspace,
            lang,
        }
    }

    /// Whether the shell's settings resolve a model this Host can call.
    ///
    /// A Host started from a scratch data directory has no shell configuration
    /// to read; that is the one case where an explicit `host.toml` provider
    /// still decides the executor.
    pub(crate) fn has_model(&self) -> bool {
        let effective = self.settings().effective();
        !effective.model.trim().is_empty() && !effective.base_url.trim().is_empty()
    }

    /// What this executor runs, for the banner a starting Host prints.
    pub(crate) fn describe(&self) -> String {
        format!("shell agent ({})", self.settings().effective().model)
    }

    /// The settings the next turn resolves.
    ///
    /// Every reader goes through this guard, so a write is either wholly
    /// visible or not at all: a query never sees a half-applied edit, and a
    /// turn never starts on a snapshot that was being replaced.
    fn settings(&self) -> std::sync::RwLockReadGuard<'_, Settings> {
        self.settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The settings this Host will write next.
    fn settings_mut(&self) -> std::sync::RwLockWriteGuard<'_, Settings> {
        self.settings
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Build the turn's agent: the same entry a desktop turn uses, with its
    /// subagents, jobs and browser attached.
    ///
    /// The session's own configuration is folded in here, the way a desktop
    /// chat's model and preset reach its build: the permission replaces the
    /// workspace default for THIS turn only, and the model and preset become
    /// the session choice. Nothing about the Host's stored settings changes.
    fn build(
        &self,
        config: &SessionConfig,
        session_id: super::ids::SessionId,
        review: Option<Arc<crate::review::store::ReviewRuntime>>,
    ) -> crate::caps::BuiltAgent {
        let mut settings = self.settings().clone();
        if !config.permission.trim().is_empty() {
            settings.workspace.agent.permission = config.permission.clone();
        }
        let model = config
            .model_ref()
            .map(|(provider, model)| ModelRef { provider, model });
        let choice = SessionChoice {
            model,
            preset: config.preset.clone(),
            workspace: Some(self.workspace.clone()),
        };
        let session = session_id.to_string();
        crate::caps::build_agent_for_session_with_subagents_jobs_and_review(
            &settings,
            &self.vault,
            &choice,
            &self.mcp,
            &session,
            &session,
            Arc::clone(&self.subagents),
            Arc::clone(&self.jobs),
            Arc::clone(&self.browser),
            None,
            review,
        )
    }

    /// Open this turn's exact-change runtime for review.
    ///
    /// The runtime is keyed by the workspace root this Host resolved and the
    /// session it serves, so it writes the same manifests the desktop's Review
    /// tab reads: a turn run in a terminal is the turn a window shows.
    ///
    /// Failing to open one disables capture for this turn rather than failing
    /// the send: a review is additive, and the turn still runs without it.
    fn start_review(
        &self,
        session_id: super::ids::SessionId,
    ) -> Option<Arc<crate::review::store::ReviewRuntime>> {
        let root = self.workspace_dir()?;
        match crate::review::store::ReviewRuntime::new(root.as_path(), &session_id.to_string()) {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                log::warn!("review capture is unavailable for session {session_id}: {error}");
                None
            }
        }
    }

    /// The choice a brand-new session resolves to.
    fn default_choice(&self) -> SessionChoice {
        SessionChoice {
            workspace: Some(self.workspace.clone()),
            ..SessionChoice::default()
        }
    }

    /// The workspace's durable memories, listed or recalled.
    ///
    /// The store is the one a turn's context assembly recalls from: the same
    /// settings, the same workspace binding and the same kernel handle the
    /// desktop's Memory pane opens. A record a tool wrote during a turn is the
    /// record this lists, because the kernel shares one handle per file inside
    /// the process; `refresh` makes a commit from another process visible.
    ///
    /// The open is async while this is called from the Host's synchronous
    /// query path, so it runs on a helper thread with its own current-thread
    /// runtime, exactly the way `caps` opens the same stores.
    fn memory_report(&self, query: Option<&str>) -> MemoryReport {
        let effective = self.settings().effective_for(&self.default_choice());
        if !effective.memory_enabled {
            return MemoryReport {
                memories: Vec::new(),
                state: "disabled".to_string(),
                error: None,
            };
        }
        let settings = self.settings();
        let root = settings
            .global
            .workspaces
            .iter()
            .find(|workspace| workspace.id == self.workspace)
            .map(|workspace| PathBuf::from(&workspace.root));
        let binding = crate::memory_scope::binding_for(&settings, root.as_deref());
        let query = query
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .map(str::to_string);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_memories(&binding, query.as_deref()));
        });
        match rx.recv() {
            Ok(Ok(report)) => report,
            Ok(Err(reason)) => MemoryReport {
                memories: Vec::new(),
                state: "init_failed".to_string(),
                error: Some(reason),
            },
            Err(_) => MemoryReport {
                memories: Vec::new(),
                state: "init_failed".to_string(),
                error: Some("the memory read thread did not return".to_string()),
            },
        }
    }

    /// One directory of the workspace this Host serves, or one file in it.
    ///
    /// The root is the Host's own workspace record - the same one a turn's
    /// file tools run with - so the pane lists the tree a turn can touch
    /// instead of whichever project the desktop opened last. Every path goes
    /// through [`crate::files::resolve`], and a file is classified before it is
    /// read, so a binary or an oversized file is refused where the desktop
    /// pane refuses it rather than being carried to a terminal as mojibake.
    /// The workspace's durable task board: the desktop Tasks card's records.
    ///
    /// The store is the platform's (`<root>/.bsh/tasks`, one JSON document per
    /// task), the same file set the desktop board and the scheduler read, and
    /// the projection is [`crate::extension::tasks::board`] - the desktop's own
    /// sectioning, so a terminal section and a desktop section cannot disagree
    /// about where a running attempt belongs. The root is the Host's own
    /// workspace record, not whichever project the desktop opened last.
    /// The directory record this Host serves, expanded.
    ///
    /// The settings name a root by workspace id, and the id this Host resolved
    /// when it opened is the one every workspace projection reads - never
    /// whichever project the desktop happened to open last.
    fn workspace_dir(&self) -> Option<std::path::PathBuf> {
        self.settings()
            .global
            .workspaces
            .iter()
            .find(|workspace| workspace.id == self.workspace)
            .map(|workspace| crate::caps::expand_path(&workspace.root))
    }

    /// The extension host's status rows, translated to the protocol roster.
    ///
    /// The rows are the host's own, so the terminal lists the packages the
    /// desktop's settings list shows: same ids, same versions, same state and
    /// the same tool names each one contributed. A host that refuses to open is
    /// reported as `failed` with its reason, never as an empty roster.
    fn extensions_report(&self) -> ExtensionsReport {
        let Some(root) = self.workspace_dir() else {
            return ExtensionsReport {
                state: "unsupported".to_string(),
                error: Some("this Host names no workspace directory".to_string()),
                ..ExtensionsReport::default()
            };
        };
        match crate::extension::host::status_roster(&self.settings(), Some(root.as_path())) {
            Ok(statuses) => {
                let mut extensions: Vec<ExtensionView> =
                    statuses.iter().map(extension_view).collect();
                extensions.sort_by(|left, right| left.name.cmp(&right.name));
                ExtensionsReport {
                    state: "ready".to_string(),
                    extensions,
                    error: None,
                }
            }
            Err(error) => ExtensionsReport {
                state: "failed".to_string(),
                extensions: Vec::new(),
                error: Some(error),
            },
        }
    }

    /// The federation node this process runs - or the store its files hold -
    /// read the way the desktop Federation tab reads it.
    ///
    /// A live node is the truth while this process runs one: the desktop's own
    /// reload prefers it for the same reason, because a peer that was just
    /// enrolled is in the node's in-memory store before the file is written.
    /// A Host with no live node reads the same files the CLI reads, so a
    /// terminal started beside a headless node still lists what that node
    /// trusts.
    fn federation_report(&self) -> FederationReport {
        let settings = self.settings();
        let profile = settings.federation_profile();
        let mut report = FederationReport {
            enabled: settings.global.federation.enabled,
            profile: profile.clone(),
            name: crate::federation::display_name(
                settings.global.federation.node_name.as_deref(),
                &profile,
            ),
            ..FederationReport::default()
        };
        // A live node in this process owns the roster, its node id and its
        // grants; reading the files beside it could report a peer the node has
        // already revoked in memory.
        if let Some(runtime) = crate::federation_node::running_profile(&profile) {
            report.node = runtime.node_id().to_string();
            if let Some(name) = runtime.node_name() {
                report.name = name;
            }
            if let Ok(store) = runtime.node().peers().read() {
                report.peers = store.iter().map(federation_peer_view).collect();
                report.state = if report.enabled {
                    "ready".to_string()
                } else {
                    "off".to_string()
                };
            } else {
                report.state = "unreadable".to_string();
                report.error = Some("this node's peer store is not readable".to_string());
            }
            return report;
        }
        let Some(peers_path) = settings.federation_peers_path() else {
            report.state = "unsupported".to_string();
            return report;
        };
        let identity_path = settings
            .global
            .federation
            .identity_path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| settings.federation_dir().join("node.key"));
        // A query must not mint an identity: a machine that has never run a
        // node has no id to name, and reporting that is honest where writing a
        // key would make every read a first launch.
        if identity_path.is_file()
            && let Ok(identity) = federation::NodeIdentity::load_or_create(&identity_path)
        {
            report.node = identity.node_id().to_string();
        }
        let store = if peers_path.is_file() {
            match federation::PeerStore::load(&peers_path) {
                Ok(store) => store,
                Err(error) => {
                    report.state = "unreadable".to_string();
                    report.error = Some(error.to_string());
                    return report;
                }
            }
        } else {
            // No store yet is a node with no peers, which is the state a fresh
            // profile is in; it is not a failure to read one.
            federation::PeerStore::default()
        };
        report.peers = store.iter().map(federation_peer_view).collect();
        report.state = match (report.enabled, report.node.is_empty()) {
            (false, _) => "off",
            (true, true) => "no_node",
            (true, false) => "ready",
        }
        .to_string();
        report
    }

    /// One session's latest sealed review, read from the workspace's store.
    ///
    /// The store is keyed by the workspace root this Host resolved, so a
    /// terminal reads the changesets the desktop's Review tab reads - the same
    /// records, because a turn captured them at the same tool boundary.
    fn review_report(&self, session: &str, change: Option<&str>) -> ReviewReport {
        let Some(root) = self.workspace_dir() else {
            return ReviewReport {
                state: "unsupported".to_string(),
                error: Some("this Host names no workspace directory".to_string()),
                session: session.to_string(),
                ..ReviewReport::default()
            };
        };
        super::review::report(root.as_path(), session, change)
    }

    /// The delegated child runs this Host's turns started.
    ///
    /// The registry is the one `caps::build_agent` hands every turn, keyed by
    /// the conversation that delegated: the same rows the desktop's task graph
    /// draws and its header lists. Nothing is started here - a query reads what
    /// the `subagent` tool already wrote, so opening the pane never spawns a
    /// child - and the transcript is deliberately left out: a header carries
    /// the title, the state and the token counts a graph draws, while the
    /// child's own rows would cost every answer the size of its whole run.
    ///
    /// A conversation the registry never saw answers an empty roster in the
    /// `ready` state: the tool was attached and nothing was delegated, which
    /// is a different fact from a build that cannot delegate at all.
    fn subagent_report(&self, session: Option<&str>) -> SubagentReport {
        let headers = match session {
            Some(session) => self.subagents.headers_for(session),
            None => self.subagents.headers(),
        };
        SubagentReport {
            state: "ready".to_string(),
            runs: headers
                .into_iter()
                .map(|header| SubagentRunView {
                    id: header.id,
                    session: header.session,
                    parent: header.parent,
                    depth: header.depth,
                    title: header.title,
                    prompt: header.prompt,
                    status: header.status.as_str().to_string(),
                    started_at_ms: header.started_at,
                    finished_at_ms: header.finished_at,
                    estimated_tokens: header.estimated_tokens,
                    prompt_tokens: header.prompt_tokens,
                    completion_tokens: header.completion_tokens,
                    revision: header.revision,
                    pending: header.pending,
                    steps: header.steps,
                    result_hint: header.result_hint,
                    error: header.error,
                })
                .collect(),
        }
    }

    /// The browser's page, as the desktop's Browser tab reads it.
    ///
    /// The handle here is the one the agent's browser tools used, so the page a
    /// turn read is the page this reports: the pane and the tools cannot hold
    /// two opinions about where the browser is or what it loaded. Nothing is
    /// fetched here - a query reads the state the tools left behind, so opening
    /// the pane never starts a request the reader did not ask for.
    fn browser_report(&self) -> BrowserView {
        let pane = self.browser.read(|browser| browser.pane_view());
        let mut view = BrowserView {
            status: pane.status.as_str().to_string(),
            address: pane.url.clone(),
            can_back: pane.can_back,
            can_forward: pane.can_forward,
            ..BrowserView::default()
        };
        if let Some(failure) = &pane.failure {
            view.error = Some(failure.message.clone());
            view.rule = failure.rule.clone();
        }
        if let Some(snapshot) = pane.snapshot {
            view.title = snapshot.title;
            view.url = snapshot.url;
            view.text = snapshot.text;
            view.text_chars = snapshot.text_chars;
            view.text_total_chars = snapshot.text_total_chars;
            view.elements_total = snapshot.elements_total;
            view.focused = snapshot.focused;
            view.elements = snapshot
                .elements
                .into_iter()
                .map(|element| BrowserElementView {
                    reference: element.reference,
                    kind: element.kind.to_string(),
                    label: element.label,
                    href: element.href,
                    value: element.value,
                    options: element.options,
                    input_type: element.input_type,
                })
                .collect();
        }
        view
    }

    fn tasks_root_report(&self) -> TaskBoardReport {
        let Some(root) = self.workspace_dir() else {
            return TaskBoardReport {
                state: "unsupported".to_string(),
                error: Some("this Host names no workspace directory".to_string()),
                ..TaskBoardReport::default()
            };
        };
        let root_text = root.to_string_lossy().into_owned();
        use crate::extension::tasks::TasksStore as _;
        match crate::extension::tasks::FileTasksStore.list(&root) {
            Ok(tasks) => TaskBoardReport {
                root: root_text,
                state: "ready".to_string(),
                sections: task_sections(&crate::extension::tasks::board(&tasks)),
                error: None,
            },
            Err(error) => TaskBoardReport {
                root: root_text,
                state: "unreadable".to_string(),
                sections: Vec::new(),
                error: Some(error.to_string()),
            },
        }
    }

    fn workspace_report(&self, path: &str) -> WorkspaceReport {
        let Some(root) = self.workspace_dir() else {
            return WorkspaceReport {
                state: "unsupported".to_string(),
                error: Some("this Host names no workspace directory".to_string()),
                ..WorkspaceReport::default()
            };
        };
        let root_text = root.to_string_lossy().into_owned();
        let path = path.trim();
        let Some(absolute) = crate::files::resolve(&root, Path::new(path)) else {
            return WorkspaceReport {
                root: root_text,
                path: path.to_string(),
                state: "refused".to_string(),
                error: Some("that path climbs out of the workspace".to_string()),
                ..WorkspaceReport::default()
            };
        };
        let meta = match std::fs::metadata(&absolute) {
            Ok(meta) => meta,
            Err(error) => {
                // A path that is not there and a directory that cannot be read
                // are different answers: the first is a name the reader may
                // have mistyped, the second is a permission problem.
                let state = if error.kind() == std::io::ErrorKind::NotFound {
                    "missing"
                } else {
                    "unreadable"
                };
                return WorkspaceReport {
                    root: root_text,
                    path: path.to_string(),
                    state: state.to_string(),
                    error: Some(error.to_string()),
                    ..WorkspaceReport::default()
                };
            }
        };
        let unreadable = |error: std::io::Error| WorkspaceReport {
            root: root_text.clone(),
            path: path.to_string(),
            state: "unreadable".to_string(),
            error: Some(error.to_string()),
            ..WorkspaceReport::default()
        };
        if meta.is_dir() {
            return match crate::files::list(&absolute, false) {
                Ok(rows) => WorkspaceReport {
                    root: root_text,
                    path: path.to_string(),
                    state: "dir".to_string(),
                    entries: rows
                        .into_iter()
                        .map(|row| WorkspaceEntryView {
                            name: row.name,
                            dir: row.dir,
                            size: row.size,
                        })
                        .collect(),
                    ..WorkspaceReport::default()
                },
                Err(error) => unreadable(error),
            };
        }
        let name = absolute
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        if !matches!(
            crate::files::kind_for(&name, meta.len()),
            crate::files::Kind::Text(_)
        ) {
            let why = if crate::files::image_extension(&name) {
                "that is a picture, and the terminal carries text only".to_string()
            } else if meta.len() > crate::files::MAX_OPEN_BYTES {
                format!(
                    "that file is {} bytes, past the {} the pane opens",
                    meta.len(),
                    crate::files::MAX_OPEN_BYTES
                )
            } else {
                "that is not a text file".to_string()
            };
            return WorkspaceReport {
                root: root_text,
                path: path.to_string(),
                state: "opaque".to_string(),
                error: Some(why),
                ..WorkspaceReport::default()
            };
        }
        let bytes = match crate::files::read(&root, Path::new(path)) {
            Ok(bytes) => bytes,
            Err(error) => return unreadable(error),
        };
        // The name said text, but a NUL in the head or bytes that are not
        // UTF-8 settle it the other way. The desktop editor refuses both for
        // the same reason: a rope would corrupt them on the next save.
        if !crate::files::is_text(&bytes) {
            return WorkspaceReport {
                root: root_text,
                path: path.to_string(),
                state: "opaque".to_string(),
                error: Some("that file is not UTF-8 text".to_string()),
                ..WorkspaceReport::default()
            };
        }
        let text = String::from_utf8(bytes).unwrap_or_default();
        let characters = text.chars().count();
        let head: String = text.chars().take(FILE_TEXT_CHARS).collect();
        let head_lines = head.lines().count();
        let content = head
            .lines()
            .take(FILE_TEXT_ROWS)
            .collect::<Vec<_>>()
            .join("\n");
        let truncated = characters > FILE_TEXT_CHARS || head_lines > FILE_TEXT_ROWS;
        WorkspaceReport {
            root: root_text,
            path: path.to_string(),
            state: "file".to_string(),
            content: Some(content),
            truncated,
            ..WorkspaceReport::default()
        }
    }

    /// The desktop Diff tab: the working copy's changed files, or one path's
    /// comparison against the same baseline the Files tree badges use.
    ///
    /// The list and the patch come from the one VCS read the desktop makes, so
    /// a terminal shows the comparison a window shows rather than a second
    /// opinion about what changed. A path is resolved against this Host's root
    /// before the VCS sees it, because the VCS would happily compare `../../x`
    /// as well - and that file is not the workspace's to show.
    fn diff_report(&self, path: &str) -> WorkspaceDiffView {
        let Some(root) = self.workspace_dir() else {
            return WorkspaceDiffView {
                state: "unsupported".to_string(),
                error: Some("this Host names no workspace directory".to_string()),
                ..WorkspaceDiffView::default()
            };
        };
        let root_text = root.to_string_lossy().into_owned();
        let status = crate::vcs::status(&root);
        let vcs = match status.kind {
            Some(crate::vcs::VcsKind::Git) => "git",
            Some(crate::vcs::VcsKind::Jj) => "jj",
            None => "",
        }
        .to_string();
        let changes: Vec<WorkspaceChangeView> = status
            .files
            .iter()
            .map(|(path, status)| WorkspaceChangeView {
                path: path.to_string_lossy().replace('\\', "/"),
                letter: status.letter().to_string(),
                label: status.label().to_string(),
            })
            .collect();
        if let Some(error) = status.error.as_deref() {
            return WorkspaceDiffView {
                root: root_text,
                state: "unreadable".to_string(),
                vcs,
                changes,
                error: Some(error.to_string()),
                ..WorkspaceDiffView::default()
            };
        }
        // No kind and no error is a root that is not a checkout at all. The
        // desktop pane refuses the comparison for the same reason: there is
        // no baseline to compare against.
        if vcs.is_empty() {
            return WorkspaceDiffView {
                root: root_text,
                state: "unknown".to_string(),
                error: Some("the workspace is not a jj or git checkout".to_string()),
                ..WorkspaceDiffView::default()
            };
        }
        let list = || WorkspaceDiffView {
            root: root_text.clone(),
            state: if changes.is_empty() {
                "clean"
            } else {
                "listed"
            }
            .to_string(),
            vcs: vcs.clone(),
            changes: changes.clone(),
            ..WorkspaceDiffView::default()
        };
        let path = path.trim();
        if path.is_empty() {
            return list();
        }
        if crate::files::resolve(&root, Path::new(path)).is_none() {
            return WorkspaceDiffView {
                state: "refused".to_string(),
                path: path.to_string(),
                error: Some("that path climbs out of the workspace".to_string()),
                ..list()
            };
        }
        match crate::vcs::diff(&root, Path::new(path)) {
            // A clean tracked file compares equal to its baseline: the VCS says
            // so with no output, which is an answer and not a failure.
            Ok(patch) if patch.trim().is_empty() => WorkspaceDiffView {
                state: "clean".to_string(),
                path: path.to_string(),
                ..list()
            },
            Ok(patch) => {
                // The same head the Files pane reads, and the same reason: a
                // patch over a whole generated file would cost every frame.
                let characters = patch.chars().count();
                let head: String = patch.chars().take(FILE_TEXT_CHARS).collect();
                let head_lines = head.lines().count();
                let content = head
                    .lines()
                    .take(FILE_TEXT_ROWS)
                    .collect::<Vec<_>>()
                    .join("\n");
                let truncated = characters > FILE_TEXT_CHARS || head_lines > FILE_TEXT_ROWS;
                WorkspaceDiffView {
                    state: "patch".to_string(),
                    path: path.to_string(),
                    patch: Some(content),
                    truncated,
                    ..list()
                }
            }
            Err(error) => WorkspaceDiffView {
                state: "unreadable".to_string(),
                path: path.to_string(),
                error: Some(error),
                ..list()
            },
        }
    }

    /// What this Host's settings resolve to, as a client reads them.
    ///
    /// The rows are built from the same `Settings` a turn's build clones, so
    /// what a terminal lists is what the next turn will use. A section name
    /// filters the rail down to one cell; an unknown name answers with the
    /// known ids instead of drawing an empty pane.
    fn settings_report(&self, section: Option<&str>) -> SettingsReport {
        let settings = self.settings();
        let roster = mcp_roster(&settings, &self.mcp);
        let sections =
            settings_sections(&settings, &self.vault, &self.workspace, &roster, self.lang);
        let Some(wanted) = section.map(str::trim).filter(|name| !name.is_empty()) else {
            return SettingsReport {
                sections,
                state: "ready".to_string(),
                error: None,
            };
        };
        let wanted = wanted.to_ascii_lowercase();
        match sections.iter().find(|section| section.section == wanted) {
            Some(found) => SettingsReport {
                sections: vec![found.clone()],
                state: "ready".to_string(),
                error: None,
            },
            None => SettingsReport {
                sections: Vec::new(),
                state: "ready".to_string(),
                error: Some(format!(
                    "no settings section '{wanted}'; known: {}",
                    sections
                        .iter()
                        .map(|section| section.section.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            },
        }
    }
}

/// Project the desktop board onto the wire: the four sections in board order,
/// each with the rows the board sorted there.
///
/// The section names are the board's own (`running`, `queued`, `scheduled`,
/// `history`), and an empty section still travels, so a client can state "no
/// running task" instead of drawing nothing.
/// One host status row as the protocol's extension view.
///
/// The fallbacks are the desktop row's: a package that declares no display
/// name is listed under its id, and the lifecycle and health strings are the
/// host's stable spellings rather than a client's paraphrase.
fn extension_view(status: &crate::extension::host::ExtensionStatus) -> ExtensionView {
    let name = if status.name.trim().is_empty() {
        status.id.clone()
    } else {
        status.name.clone()
    };
    ExtensionView {
        id: status.id.clone(),
        name,
        version: status.version.clone(),
        versions: status.versions.clone(),
        runtime: status.runtime.clone(),
        trust: status.trust.clone(),
        signature: status.signature.clone(),
        source: status.source.clone(),
        state: status.state.as_str().to_string(),
        health: status.health.as_str().to_string(),
        configured: status.configured,
        applied: status.applied,
        tools: status.tools.clone(),
        commands: status.commands.clone(),
        panels: status.panels.clone(),
        error: status.error.clone(),
    }
}

/// One peer's trust record, translated to the contract's view.
///
/// The state spellings are the desktop's: the same `TrustState` maps to the same
/// word in the Federation tab, so a terminal row and a window row cannot
/// describe the same peer differently. Grants carry their scope because a
/// capability name alone does not say what a trusted peer may touch.
fn federation_peer_view(record: &federation::PeerRecord) -> FederationPeerView {
    FederationPeerView {
        node: record.node.to_string(),
        name: record.note.clone(),
        state: match record.state {
            federation::TrustState::Invited => "invited",
            federation::TrustState::Pending => "pending",
            federation::TrustState::Trusted => "trusted",
            federation::TrustState::Suspended => "suspended",
            federation::TrustState::Revoked => "revoked",
        }
        .to_string(),
        grants: record
            .grants
            .iter()
            .map(|grant| FederationGrantView {
                capability: grant.capability.to_string(),
                operations: grant.operations.clone(),
                resources: grant.resources.clone(),
                workspaces: grant.workspaces.clone(),
                expires_ms: grant.expires_at_ms,
            })
            .collect(),
        enrolled_ms: record.enrolled_at_ms,
        expires_ms: record.expires_at_ms,
    }
}

fn task_sections(board: &crate::extension::tasks::Board) -> Vec<TaskSectionView> {
    use crate::extension::tasks::BoardSectionKind;
    board
        .sections
        .iter()
        .map(|section| TaskSectionView {
            section: match section.kind {
                BoardSectionKind::Running => "running",
                BoardSectionKind::Queued => "queued",
                BoardSectionKind::Scheduled => "scheduled",
                BoardSectionKind::History => "history",
            }
            .to_string(),
            rows: section
                .rows
                .iter()
                .map(|row| TaskRowView {
                    id: row.id.clone(),
                    title: row.title.clone(),
                    tier: snake(&row.tier),
                    status: snake(&row.status),
                    host: host_label(&row.host),
                    session: row.session.clone(),
                    parent: row.parent.clone().unwrap_or_default(),
                    deps: row.deps.clone(),
                    created_ms: row.created_ms,
                    next_fire_ms: row.next_fire_ms,
                    attempts: row.attempts,
                    verification: snake(&row.verification),
                })
                .collect(),
        })
        .collect()
}

/// One task record as the expanded card reads it.
///
/// Every closed vocabulary goes through [`snake`], the same conversion the
/// board rows use, so the detail and the row agree on the word for a state
/// instead of two spellings of the same lifecycle.
fn task_detail_report(root: &str, task: &crate::extension::tasks::Task) -> TaskDetailReport {
    TaskDetailReport {
        root: root.to_string(),
        state: "ready".to_string(),
        id: task.id.clone(),
        title: task.title.clone(),
        intent: task.intent.clone(),
        tier: snake(&task.tier),
        status: snake(&task.status),
        host: host_label(&task.host),
        parent: task.parent.clone().unwrap_or_default(),
        target: task.target.clone().unwrap_or_default(),
        command: task.command.clone().unwrap_or_default(),
        trigger: trigger_label(&task.trigger),
        next_fire_ms: task.trigger.next_fire_ms(),
        created_ms: task.created_ms,
        updated_ms: task.updated_ms,
        deps: task.deps.clone(),
        max_steps: task.budget.max_steps,
        max_tokens: task.budget.max_tokens,
        deadline_ms: task.budget.deadline_ms,
        retry: snake(&task.retry.mode),
        max_attempts: task.retry.max_attempts,
        verification: snake(&task.verification.status),
        verification_contract: match &task.verification_contract {
            Some(crate::extension::tasks::VerificationContract::Command { .. }) => {
                "command".to_string()
            }
            Some(crate::extension::tasks::VerificationContract::Artifact { .. }) => {
                "artifact".to_string()
            }
            Some(crate::extension::tasks::VerificationContract::Review) => "review".to_string(),
            Some(crate::extension::tasks::VerificationContract::Human) => "human".to_string(),
            None => "none".to_string(),
        },
        attempts: task
            .attempts
            .iter()
            .map(|attempt| TaskAttemptView {
                id: attempt.id.clone(),
                tier: snake(&attempt.tier),
                status: snake(&attempt.status),
                run: attempt.run.clone().unwrap_or_default(),
                started_ms: attempt.started_ms,
                ended_ms: attempt.ended_ms,
                result: attempt.result.clone().unwrap_or_default(),
                error: attempt.error.clone().unwrap_or_default(),
                change_set: attempt.change_set_id.clone().unwrap_or_default(),
                verification: snake(&attempt.verification.status),
            })
            .collect(),
        error: None,
    }
}

/// How a task fires, in the words the card uses.
fn trigger_label(trigger: &crate::extension::tasks::Trigger) -> String {
    match trigger {
        crate::extension::tasks::Trigger::Now => "now".to_string(),
        crate::extension::tasks::Trigger::At { at_ms } => format!("at {at_ms}"),
        crate::extension::tasks::Trigger::Every { every_seconds, .. } => {
            format!("every {every_seconds}s")
        }
    }
}

/// The host label one board row carries.
///
/// `Host` is a tagged record (a peer mirror names the owning node), so it does
/// not flatten to a bare string the way the plain enums do: matching here is
/// what keeps a peer task reading as `peer` rather than as an object.
fn host_label(host: &crate::extension::tasks::Host) -> String {
    match host {
        crate::extension::tasks::Host::Local => "local".to_string(),
        crate::extension::tasks::Host::Peer { .. } => "peer".to_string(),
    }
}

/// One record enum's snake_case spelling, taken from its own serde name.
///
/// Matching every variant here would be a second copy of the record format that
/// drifts the first time a status is added; the record's serializer is the one
/// the store, the desktop board and the scheduler already agree on.
fn snake<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The characters a `Files` query carries for one text file.
///
/// A terminal pane pages through what it is given, so the Host carries the
/// head of a file and says it was cut rather than streaming megabytes into one
/// response frame.
const FILE_TEXT_CHARS: usize = 64_000;

/// The rows a `Files` query carries for one text file.
const FILE_TEXT_ROWS: usize = 4_000;

/// How many records one listing returns.
///
/// The desktop pane caps each store at the same figure, so a workspace with a
/// long history lists what the reader can still manage rather than the whole
/// file over one frame.
const MEMORY_ROWS: usize = 500;

/// Read the kernel on a runtime of its own: list the visible records, or run
/// the hybrid recall for `query`.
fn read_memories(
    binding: &crate::memory_scope::MemoryBinding,
    query: Option<&str>,
) -> Result<MemoryReport, String> {
    use agent::memory::MemoryRepository as _;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not start the memory runtime: {error}"))?;
    rt.block_on(async {
        let store = agent::memory::FileMemory::open_scoped(
            &binding.workspace_store,
            binding.storage_scope.clone(),
            Some(binding.storage_scope.clone()),
        )
        .await
        .map_err(|error| {
            format!(
                "could not open {}: {error}",
                binding.workspace_store.display()
            )
        })?;
        // A second process may have committed since this handle last read.
        store.refresh().await.map_err(|error| error.to_string())?;
        match query {
            None => {
                let mut records = store.visible_records();
                // The pane lists newest first: the record a reader just wrote
                // is the row they opened the pane to find.
                records.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
                records.truncate(MEMORY_ROWS);
                let empty = records.is_empty();
                Ok(MemoryReport {
                    memories: records
                        .iter()
                        .map(|record| memory_view(record, None, String::new()))
                        .collect(),
                    state: if empty {
                        "empty".to_string()
                    } else {
                        "ready".to_string()
                    },
                    error: None,
                })
            }
            Some(query) => {
                let outcome = store
                    .repository()
                    .recall(query, &agent::memory::RecallRequest::default())
                    .await
                    .map_err(|error| error.to_string())?;
                let memories: Vec<super::contract::MemoryView> = outcome
                    .hits
                    .iter()
                    .map(|hit| {
                        let reasons = hit
                            .features
                            .iter()
                            .map(|feature| format!("{} {:.2}", feature.name, feature.value))
                            .collect::<Vec<_>>()
                            .join(" · ");
                        memory_view(&hit.record, Some(hit.score), reasons)
                    })
                    .collect();
                let empty = memories.is_empty();
                Ok(MemoryReport {
                    memories,
                    state: if empty {
                        "empty".to_string()
                    } else {
                        "ready".to_string()
                    },
                    // A skip explains why searching this store returned
                    // nothing; without one, an empty result is just empty.
                    error: outcome.skip,
                })
            }
        }
    })
}

/// One record as the wire carries it, plus its recall explanation when the
/// row came from a search.
fn memory_view(
    record: &agent::memory::MemoryRecord,
    score: Option<f64>,
    reasons: String,
) -> super::contract::MemoryView {
    let updated_at = chrono::DateTime::from_timestamp_millis(record.updated_at_ms as i64)
        .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
    super::contract::MemoryView {
        id: record.id.clone(),
        content: record.content.clone(),
        kind: record.kind.label().to_string(),
        scope: scope_level(&record.scope).to_string(),
        scope_id: scope_id(&record.scope),
        lifecycle: lifecycle_label(record.status).to_string(),
        source: source_label(record.provenance.source).to_string(),
        revision: record.revision,
        updated_at,
        score,
        reasons,
    }
}

/// The scope level label the desktop's Memory pane prints.
fn scope_level(scope: &agent::memory::MemoryScope) -> &'static str {
    match scope {
        agent::memory::MemoryScope::User { .. } => "user",
        agent::memory::MemoryScope::Workspace { .. } => "workspace",
        agent::memory::MemoryScope::Session { .. } => "session",
        agent::memory::MemoryScope::Agent { .. } => "agent",
    }
}

/// The most specific scope id, shortened so a UUID never floods a row.
fn scope_id(scope: &agent::memory::MemoryScope) -> String {
    let id = match scope {
        agent::memory::MemoryScope::User { user } => user,
        agent::memory::MemoryScope::Workspace { workspace } => workspace,
        agent::memory::MemoryScope::Session { session, .. } => session,
        agent::memory::MemoryScope::Agent { agent, .. } => agent,
    };
    id.chars().take(8).collect()
}

/// The lifecycle label the desktop's Memory pane prints.
fn lifecycle_label(status: agent::memory::Lifecycle) -> &'static str {
    match status {
        agent::memory::Lifecycle::Candidate => "candidate",
        agent::memory::Lifecycle::Active => "active",
        agent::memory::Lifecycle::Superseded => "superseded",
        agent::memory::Lifecycle::Expired => "expired",
        agent::memory::Lifecycle::Tombstoned => "tombstoned",
    }
}

/// The provenance label the desktop's Memory pane prints.
fn source_label(source: agent::memory::EvidenceSource) -> &'static str {
    match source {
        agent::memory::EvidenceSource::UserStatement => "user_statement",
        agent::memory::EvidenceSource::ModelInference => "model_inference",
        agent::memory::EvidenceSource::ToolEvidence => "tool_evidence",
        agent::memory::EvidenceSource::Legacy => "legacy",
        agent::memory::EvidenceSource::System => "system",
    }
}

/// Connect the MCP servers this build may run, on a runtime of its own.
///
/// The Host opens inside its async runtime and a connection blocks on a handshake
/// or a child's first frame, so the pool is reconciled on a helper thread with a
/// current-thread runtime rather than on the caller's. The egress gate is
/// installed before the first handshake, the way a desktop build installs it, so
/// a host the policy does not name opens no socket here either.
///
/// The gate attributes a decision to a session and a preset. A Host connects
/// before it has a session, so the roster is opened under the workspace default
/// and the desktop's own labels; the per-turn gate `caps` installs replaces it
/// before a tool call, so no request the model makes asks this first one.
fn connect_mcp(settings: &Settings, choice: &SessionChoice) -> (crate::mcp::McpPool, Vec<String>) {
    let settings = settings.clone();
    let choice = choice.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(connect_on_own_runtime(&settings, &choice));
    });
    match receiver.recv_timeout(Duration::from_secs(MCP_CONNECT_SECS)) {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => {
            log::warn!("mcp roster unavailable: {error}");
            (crate::mcp::McpPool::default(), Vec::new())
        }
        Err(error) => {
            log::warn!("mcp roster did not finish connecting: {error}");
            (crate::mcp::McpPool::default(), Vec::new())
        }
    }
}

/// The blocking half of [`connect_mcp`].
fn connect_on_own_runtime(
    settings: &Settings,
    choice: &SessionChoice,
) -> Result<(crate::mcp::McpPool, Vec<String>), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("async runtime: {error}"))?;
    let effective = settings.effective_for(choice);
    let policy = crate::caps::policy_for_effective(settings, &effective);
    let mut pool = crate::mcp::McpPool::default();
    pool.set_egress_gate(Arc::new(agent::policy::EgressGate::new(
        policy,
        Arc::new(agent::policy::SystemResolver),
        "host",
        effective.preset_id.clone(),
        "mcp",
    )));
    let servers = settings.mcp_servers();
    let extensions = crate::mcp::extension_servers(settings);
    let failed = pool.sync_all(&runtime, &servers, &extensions);
    Ok((pool, failed))
}

/// The MCP roster a client may list, name-ordered.
///
/// A settings server is listed whether it is enabled or not, because the pane
/// shows a switched-off row too; an extension's server is listed only when the
/// trust gate released it, since that gate is exactly what makes the pool see
/// it. Tools and the failure reason come from the live pool, so a client never
/// claims a tool the turn cannot call.
fn mcp_roster(settings: &Settings, pool: &crate::mcp::McpPool) -> Vec<McpServerView> {
    let mut rows: Vec<McpServerView> = settings
        .mcp_servers()
        .into_iter()
        .map(|server| McpServerView {
            name: server.name.clone(),
            transport: crate::mcp::transport_label(&server.transport),
            enabled: server.enabled,
            extension: false,
            tools: pool.tool_names(&server.name).unwrap_or_default(),
            error: pool.error(&server.name).map(str::to_string),
        })
        .collect();
    rows.extend(
        crate::mcp::extension_servers(settings)
            .into_iter()
            .map(|server| McpServerView {
                name: server.name.clone(),
                transport: crate::mcp::transport_label(&server.transport),
                enabled: true,
                extension: true,
                tools: pool.tool_names(&server.name).unwrap_or_default(),
                error: pool.error(&server.name).map(str::to_string),
            }),
    );
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    rows
}

/// The settings a turn resolves from, as the settings rail's rows.
///
/// The sections are the rail's cells that decide what a turn is - the model and
/// its credential, the workspace and its confinement, the tool switches, the
/// local search endpoint, the vault's state and the machine preferences - and
/// every value is the EFFECTIVE one, folded exactly as `caps` folds it when it
/// builds a turn. A reader who asks why a turn called one endpoint or ran under
/// one permission reads the answer here without opening `settings.toml`, and a
/// terminal and the window cannot disagree because this is the same resolution
/// the turn uses.
///
/// Secrets never appear: a credential row names where the key comes from - the
/// vault, the environment, or nothing - and never the key itself.
fn settings_sections(
    settings: &Settings,
    vault: &Vault,
    workspace: &str,
    mcp_rows: &[McpServerView],
    lang: Lang,
) -> Vec<SettingsSectionView> {
    let effective = settings.effective_for(&SessionChoice {
        workspace: Some(workspace.to_string()),
        ..SessionChoice::default()
    });
    let general = &settings.global.general;
    let search = &settings.global.web_search;
    let (credential, credential_source) = credential_state(&effective, vault, lang);
    let model_label = if effective.model_name.trim().is_empty() {
        shown(&effective.model)
    } else {
        effective.model_name.trim().to_string()
    };
    let protocol = if effective.api_mode.trim().is_empty() {
        "follow model".to_string()
    } else {
        effective.api_mode.trim().to_string()
    };
    let max_tokens = effective
        .max_tokens
        .map(|tokens| tokens.to_string())
        .unwrap_or_else(|| "model default".to_string());
    let reasoning = effective
        .reasoning_effort
        .clone()
        .unwrap_or_else(|| "engine default".to_string());
    let model_section = SettingsSectionView {
        section: "model".to_string(),
        rows: vec![
            row("provider", shown(&effective.provider), "settings"),
            row("model", model_label, "settings"),
            row("api model", shown(&effective.model), "settings"),
            row("endpoint", shown(&effective.base_url), "settings"),
            row("credential", credential, credential_source),
            row("protocol", protocol, "settings"),
            editable_row(
                "temperature",
                "temperature",
                format!("{:.1}", effective.temperature),
                "settings",
            ),
            editable_row("max-tokens", "max tokens", max_tokens, "settings"),
            row("reasoning", reasoning, "settings"),
        ],
    };

    let permission = &effective.permission;
    let mut workspace_rows = vec![
        row("root", shown(&effective.root), "workspace"),
        row("permission", permission.effective.clone(), "workspace"),
    ];
    if let Some(ceiling) = permission.reduced_by.as_deref() {
        workspace_rows.push(row(
            "permission ceiling",
            format!(
                "{} · asked {}",
                shown(ceiling),
                shown(&permission.requested)
            ),
            "preset",
        ));
    }
    let preset_label = if effective.presets_disabled {
        "presets off".to_string()
    } else {
        shown(&effective.preset_id)
    };
    let sandbox = if effective.sandbox.heel {
        "heel"
    } else {
        "unconfined"
    };
    let budget = if effective.context_budget == 0 {
        "off".to_string()
    } else {
        format!("{} tokens", effective.context_budget)
    };
    workspace_rows.extend([
        row("preset", preset_label, "workspace"),
        row("sandbox", sandbox, "workspace"),
        editable_row("context-budget", "context budget", budget, "workspace"),
        editable_row(
            "verify-command",
            "verify command",
            effective
                .verify_command
                .clone()
                .unwrap_or_else(|| "none".to_string()),
            "workspace",
        ),
    ]);
    let workspace_section = SettingsSectionView {
        section: "workspace".to_string(),
        rows: workspace_rows,
    };

    let disabled = if effective.disabled_tools.is_empty() {
        "none".to_string()
    } else {
        effective.disabled_tools.join(", ")
    };
    let mcp_tools: usize = mcp_rows.iter().map(|server| server.tools.len()).sum();
    let tools_section = SettingsSectionView {
        section: "tools".to_string(),
        rows: vec![
            editable_row("shell", "shell", flag(effective.bash_enabled), "settings"),
            editable_row(
                "files",
                "files",
                flag(effective.file_tools_enabled),
                "settings",
            ),
            editable_row(
                "hosted-web-search",
                "hosted web search",
                flag(effective.web_tools_enabled),
                "settings",
            ),
            editable_row(
                "parallel-tool-calls",
                "parallel tool calls",
                flag(effective.parallel_tool_calls),
                "settings",
            ),
            row("memory", flag(effective.memory_enabled), "settings"),
            row(
                "skills",
                format!("{} enabled", effective.enabled_skills().count()),
                "settings",
            ),
            row(
                "mcp",
                format!("{} server(s), {mcp_tools} tool(s)", mcp_rows.len()),
                "settings",
            ),
            row("disabled tools", disabled, "settings"),
        ],
    };

    // The desktop's Agent loop cell: the one wait a model request gets, its
    // retry budget and the shell's deadline. The dialog edits them on one page
    // beside the tool switches, and all three live in the same `agent` block a
    // turn reads, so the rail lists them together here instead of inventing a
    // cell the dialog does not have.
    let agent_loop_section = SettingsSectionView {
        section: "agent-loop".to_string(),
        rows: vec![
            editable_row(
                "llm-timeout",
                "model timeout",
                format!("{} s", effective.llm_timeout_secs),
                "settings",
            ),
            editable_row(
                "llm-retries",
                "model retries",
                effective.llm_max_retries.to_string(),
                "settings",
            ),
            editable_row(
                "shell-timeout",
                "shell timeout",
                format!("{} s", effective.shell_timeout_secs),
                "settings",
            ),
        ],
    };

    let search_method = if search.method.trim().is_empty() {
        "POST".to_string()
    } else {
        search.method.trim().to_uppercase()
    };
    let web_search_section = SettingsSectionView {
        section: "web-search".to_string(),
        rows: vec![
            row("method", search_method, "settings"),
            editable_row(
                "web-search-base-url",
                "base url",
                shown(&search.base_url),
                "settings",
            ),
            row("path", shown(&search.path), "settings"),
            editable_row(
                "web-search-results",
                "results",
                search.results_per_search.to_string(),
                "settings",
            ),
            row(
                "credential",
                vault_entry_state(vault, crate::vault::WEB_SEARCH_KEY),
                "vault",
            ),
        ],
    };

    let vault_state = if !vault.exists() {
        "no vault"
    } else if vault.is_unlocked() {
        "unlocked"
    } else if vault.is_app_available() {
        "app access"
    } else {
        "sealed"
    };
    let entries = if vault.is_unlocked() {
        vault
            .entries()
            .map(|entries| entries.len().to_string())
            .unwrap_or_else(|_| "unreadable".to_string())
    } else {
        "sealed".to_string()
    };
    let vault_path = vault
        .path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unset".to_string());
    let provider_entry = if effective.provider.trim().is_empty() {
        "unset".to_string()
    } else {
        vault_entry_state(vault, &crate::vault::provider_key(&effective.provider))
    };
    let vault_section = SettingsSectionView {
        section: "vault".to_string(),
        rows: vec![
            row("state", vault_state, "vault"),
            row("entries", entries, "vault"),
            row("provider key", provider_entry, "vault"),
            row("path", vault_path, "vault"),
        ],
    };

    let general_section = SettingsSectionView {
        section: "general".to_string(),
        rows: vec![
            row("language", shown(&general.lang), "settings"),
            row("theme", shown(&general.theme_mode), "settings"),
            row("transcript", shown(&general.transcript_view), "settings"),
            row("composer enter", shown(&general.composer_enter), "settings"),
            row("developer tools", flag(general.developer_tools), "settings"),
            row(
                "content font size",
                format!("{} px", general.content_font_size),
                "settings",
            ),
        ],
    };

    vec![
        model_section,
        workspace_section,
        tools_section,
        agent_loop_section,
        web_search_section,
        vault_section,
        general_section,
    ]
}

/// One row, with its label, value and provenance in the shapes a pane draws.
fn row(label: &str, value: impl Into<String>, source: &str) -> SettingsRowView {
    SettingsRowView {
        label: label.to_string(),
        value: value.into(),
        source: source.to_string(),
        key: None,
    }
}

/// One row a client may write, carrying the key [`Self::update_settings`]
/// takes for it.
///
/// The key is the row's identity on the wire, not a second name a client keeps
/// on its side: `/settings` draws this row, and the thing the reader points at
/// is exactly what a write sends.
fn editable_row(key: &str, label: &str, value: impl Into<String>, source: &str) -> SettingsRowView {
    SettingsRowView {
        key: Some(key.to_string()),
        ..row(label, value, source)
    }
}

/// A value a reader reads as absent rather than as an empty cell.
fn shown(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "unset".to_string()
    } else {
        trimmed.to_string()
    }
}

/// A switch's word.
fn flag(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// Whether the vault exists but cannot answer for its entries.
///
/// A sealed vault is not an empty one: a row must not report "not stored" for
/// an entry this process is not allowed to look for.
fn sealed(vault: &Vault) -> bool {
    vault.exists() && !vault.is_unlocked() && !vault.is_app_available()
}

/// One entry's state, named the way the vault pane names it.
fn vault_entry_state(vault: &Vault, name: &str) -> String {
    if vault.contains(name) {
        "stored".to_string()
    } else if sealed(vault) {
        "unknown · vault sealed".to_string()
    } else {
        "not stored".to_string()
    }
}

/// Where a provider's credential comes from, never the credential itself.
fn credential_state(
    effective: &crate::settings::Effective,
    vault: &Vault,
    lang: Lang,
) -> (String, &'static str) {
    if effective.provider.trim().is_empty() {
        return ("unset".to_string(), "unset");
    }
    match crate::caps::provider_credential(
        &effective.provider,
        effective.api_key_vault.as_deref(),
        vault,
    ) {
        (_, Some(error)) => (error.message(lang), "vault"),
        (secret, None) if !secret.is_empty() => ("stored in the vault".to_string(), "vault"),
        // With no vault holding a key the provider is expected to read one from
        // the environment; saying so is the honest answer, because this process
        // never looks at the variable itself.
        (_, None) => ("environment".to_string(), "env"),
    }
}

#[async_trait]
impl TurnExecutor for AgentExecutor {
    fn settings(&self, section: Option<&str>) -> SettingsReport {
        self.settings_report(section)
    }

    /// Write the fields the report marks editable, then answer with the rail
    /// the next turn resolves.
    ///
    /// The order is deliberate: apply every edit to a COPY, write that copy to
    /// the documents, and only then swap it into the lock. A refused value
    /// therefore changes neither file nor live state, and a failed write leaves
    /// the Host resolving exactly what it resolved before - the reader is told
    /// the settings did not change instead of watching the pane report a value
    /// that never landed.
    ///
    /// A key the desktop's form does not edit is refused here too, because the
    /// Host owns what a value may be: a client cannot invent a field by naming
    /// one, and a removed field is answered with the list that replaced it.
    fn update_settings(&self, edits: &[SettingEdit]) -> Result<SettingsReport, String> {
        if edits.is_empty() {
            return Err("no settings were named; send at least one key".to_string());
        }
        let mut next = self.settings().clone();
        for edit in edits {
            crate::settings::edit::apply(&mut next, &edit.key, &edit.value)?;
        }
        let root = self.workspace_dir().ok_or_else(|| {
            "this Host names no workspace directory, so its settings have nowhere to land"
                .to_string()
        })?;
        next.save_for_root(&root)
            .map_err(|error| format!("settings write failed: {error}"))?;
        // The swap happens under the same lock every reader takes, so a query
        // that arrives mid-write sees the old document or the new one, never a
        // blend of the two.
        *self.settings_mut() = next;
        Ok(self.settings_report(None))
    }

    fn memories(&self, query: Option<&str>) -> MemoryReport {
        self.memory_report(query)
    }

    fn files(&self, path: &str) -> WorkspaceReport {
        self.workspace_report(path)
    }

    fn diff(&self, path: &str) -> WorkspaceDiffView {
        self.diff_report(path)
    }

    fn tasks(&self) -> TaskBoardReport {
        self.tasks_root_report()
    }

    /// One task's record, read from the store the board is read from.
    ///
    /// The desktop expands a Tasks row into the objective, the dependency
    /// edges, the attempt history and the caps each attempt ran under; this
    /// answers the same record so a terminal opens the same task rather than a
    /// second reading of the row. An id a path cannot name is `unknown`, not
    /// `unreadable`: it addresses no record, which is what the store would
    /// answer if it could be asked.
    fn task_detail(&self, id: &str) -> TaskDetailReport {
        let id = id.trim();
        let Some(root) = self.workspace_dir() else {
            return TaskDetailReport {
                state: "unsupported".to_string(),
                id: id.to_string(),
                error: Some("this Host names no workspace directory".to_string()),
                ..TaskDetailReport::default()
            };
        };
        let root_text = root.to_string_lossy().into_owned();
        if id.is_empty() {
            return TaskDetailReport {
                root: root_text,
                state: "unknown".to_string(),
                error: Some("no task id was named".to_string()),
                ..TaskDetailReport::default()
            };
        }
        use crate::extension::tasks::TasksStore as _;
        match crate::extension::tasks::FileTasksStore.load(&root, id) {
            Ok(Some(task)) => task_detail_report(&root_text, &task),
            Ok(None) => TaskDetailReport {
                root: root_text,
                state: "unknown".to_string(),
                id: id.to_string(),
                error: Some(format!("no task in the store carries the id `{id}`")),
                ..TaskDetailReport::default()
            },
            // A separator or a parent reference addresses no record rather
            // than a record that went missing, so it is answered as unknown.
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => TaskDetailReport {
                root: root_text,
                state: "unknown".to_string(),
                id: id.to_string(),
                error: Some(format!("`{id}` is not a task id: {error}")),
                ..TaskDetailReport::default()
            },
            Err(error) => TaskDetailReport {
                root: root_text,
                state: "unreadable".to_string(),
                id: id.to_string(),
                error: Some(error.to_string()),
                ..TaskDetailReport::default()
            },
        }
    }

    fn extensions(&self) -> ExtensionsReport {
        self.extensions_report()
    }

    fn federation(&self) -> FederationReport {
        self.federation_report()
    }

    fn review(&self, session: &str, change: Option<&str>) -> ReviewReport {
        self.review_report(session, change)
    }

    fn subagents(&self, session: Option<&str>) -> SubagentReport {
        self.subagent_report(session)
    }

    fn browser(&self) -> BrowserView {
        self.browser_report()
    }

    fn default_config(&self) -> SessionConfig {
        let effective = self.settings().effective_for(&self.default_choice());
        SessionConfig {
            // A fresh session names no model and no preset: it follows the
            // machine default and the workspace's preset state, and the first
            // time the reader picks one it becomes the session's own fact.
            provider: None,
            model: None,
            preset: None,
            permission: effective.permission.effective.clone(),
            plan_mode: false,
        }
    }

    fn catalogue(&self) -> CatalogueView {
        let settings = self.settings();
        let mut providers: Vec<ProviderModels> = settings
            .global
            .providers
            .iter()
            .filter(|(_, provider)| !provider.models.is_empty())
            .map(|(id, provider)| ProviderModels {
                provider: id.clone(),
                models: provider.models.keys().cloned().collect(),
            })
            .collect();
        // The provider the default model resolves to leads the list, the way
        // the picker opens on the choice in force rather than on id order.
        if let Some(active) = settings.effective_for(&self.default_choice()).model_ref
            && let Some(index) = providers
                .iter()
                .position(|provider| provider.provider == active.provider)
        {
            let first = providers.remove(index);
            providers.insert(0, first);
        }
        // The skills this build registers, name-ordered like the palette's
        // section: the rows a terminal offers and the gestures a turn can
        // resolve are the same set, because both read this resolution.
        let mut skills: Vec<SkillView> = settings
            .effective_for(&self.default_choice())
            .enabled_skills()
            .map(|skill| SkillView {
                name: skill.name.clone(),
                description: skill.description.clone(),
                user_only: skill.user_only,
            })
            .collect();
        skills.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.name.cmp(&right.name))
        });
        CatalogueView {
            providers,
            presets: crate::preset_catalogue::catalogue(&settings.global)
                .into_iter()
                .map(|entry| entry.id)
                .collect(),
            skills,
            mcp: mcp_roster(&settings, &self.mcp),
            // The folder `open` resolved for this Host, read back from the
            // workspace record its id names: an attached client lists the tree
            // the file tools may touch instead of guessing from its own cwd.
            workspace_root: settings
                .global
                .workspaces
                .iter()
                .find(|workspace| workspace.id == self.workspace)
                .map(|workspace| workspace.root.clone())
                .unwrap_or_default(),
        }
    }

    fn configure(
        &self,
        current: &SessionConfig,
        change: &SessionConfigChange,
    ) -> Result<SessionConfig, String> {
        let next = apply_config_change(current, change)?;
        let catalogue = self.catalogue();
        if change.provider.is_some() || change.model.is_some() {
            let Some((provider, model)) = next.model_ref() else {
                return Err("a model change must name both the provider and the model".to_string());
            };
            let known = catalogue
                .providers
                .iter()
                .any(|entry| entry.provider == provider && entry.models.contains(&model));
            if !known {
                return Err(format!(
                    "provider '{provider}' has no model '{model}' in the configured providers"
                ));
            }
        }
        if let Some(preset) = &change.preset
            && !catalogue.presets.contains(preset)
        {
            return Err(format!("no preset '{preset}' is configured"));
        }
        Ok(next)
    }

    /// Fold this session's older history with the same summarizer a desktop
    /// `/compact` runs.
    ///
    /// The Host owns the journal and the projection; only an executor carries
    /// a model, so the checkpoint text has to come from a request. Seeding the
    /// engine with the projected transcript is what makes the fold land on the
    /// span a client showed: a checkpoint is written from those rows, never
    /// from an engine transcript the Host does not hold.
    async fn compact(&self, context: CompactContext) -> Result<CompactionOutput, TurnError> {
        let built = self.build(&context.config, context.session_id, None);
        if let Some(error) = &built.credential_error {
            return Err(TurnError::new(error.message(self.lang)));
        }
        if built.effective.model.trim().is_empty() || built.effective.base_url.trim().is_empty() {
            return Err(TurnError::new(
                "no model resolved for this session; choose a model in bsh so the Host can compact it",
            ));
        }
        let agent = built.agent;
        {
            let mut session = agent.session();
            session.restore_messages(Vec::new());
            for message in &context.history {
                match message.role {
                    MessageRole::User => session.add_user(message.text.clone()),
                    MessageRole::Assistant => {
                        if !message.text.is_empty() {
                            session.add_assistant(message.text.clone());
                        }
                    }
                }
            }
        }
        // A manual compaction does not wait for context pressure, so it
        // retains no tail. Cancellation is by dropping this future, which is
        // why the signal handed to the summarizer is never set.
        let signal = std::sync::atomic::AtomicBool::new(false);
        let outcome =
            agent
                .compact_now(COMPACT_RETAIN, &signal)
                .await
                .map_err(|error| match error {
                    agent::AgentError::Stopped => TurnError::new("the compaction was cancelled"),
                    other => TurnError::new(other.to_string()),
                })?;
        // The summarizer refuses before opening a request when no span is
        // safe to fold. That is a refusal, not a checkpoint: reporting it as
        // one would install a summary of rows that still exist.
        let Some(report) = outcome else {
            return Err(TurnError::new(
                "nothing in this session's history is safe to fold into a checkpoint",
            ));
        };
        Ok(CompactionOutput {
            summary: report.summary,
            folded: u64::try_from(report.replaced).unwrap_or(u64::MAX),
        })
    }

    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError> {
        // The turn's exact-change runtime: the file tools capture every write
        // through it, and [`SealOnDrop`] seals the changeset when this turn
        // ends - by an answer, a refusal or an error, the way the desktop seals
        // at settle. A turn that wrote nothing leaves no manifest behind.
        let review = self.start_review(context.session_id);
        let _seal = review.clone().map(SealOnDrop);
        let built = self.build(&context.config, context.session_id, review);
        // No turn opens without a credential: a locked vault names itself
        // instead of sending the request unauthenticated.
        if let Some(error) = &built.credential_error {
            return Err(TurnError::new(error.message(self.lang)));
        }
        if built.effective.model.trim().is_empty() || built.effective.base_url.trim().is_empty() {
            return Err(TurnError::new(
                "no model resolved for this turn; choose a model in bsh so the Host can run it",
            ));
        }
        // An unconfined turn runs the model's commands with no workspace
        // boundary. The operator hears it from the approval card instead of
        // finding it in the audit log afterwards.
        let permission = built.effective.permission.effective.clone();
        if !crate::caps::is_confined(&permission) {
            let request = ApprovalRequest {
                summary: format!(
                    "run an unconfined turn ({permission}) with {} tools",
                    built.agent.all_tools().len()
                ),
                action: ApprovalTarget {
                    tool: "turn".to_string(),
                    action: "execute".to_string(),
                    resource: Some(built.effective.root.clone()),
                },
                expires_in_secs: TURN_APPROVAL_SECS,
            };
            let outcome = context.approvals.request(request).await.map_err(|error| {
                TurnError::new(format!(
                    "this turn needs the operator's approval, and {error}"
                ))
            })?;
            if !outcome.is_approved() {
                return Err(TurnError::new("the operator rejected this unconfined turn"));
            }
        }

        // Seed the engine's own session with the transcript the Host projected,
        // so this turn reads what earlier turns said. Reasoning and tool
        // traffic stay out: the projection carries text, and a resumed chat
        // must not replay either as a fresh model turn. The guard is scoped:
        // holding the session lock across the stream would make the turn's
        // future non-Send and block the Host's task spawn.
        {
            let mut session = built.agent.session();
            session.restore_messages(Vec::new());
            for message in &context.history {
                match message.role {
                    MessageRole::User => session.add_user(message.text.clone()),
                    MessageRole::Assistant => {
                        if !message.text.is_empty() {
                            session.add_assistant(message.text.clone());
                        }
                    }
                }
            }
        }

        // The reader's line goes through the same composition a desktop turn
        // uses, so an @reference and plan mode read the same in a terminal as
        // they do in a window.
        let text = crate::state::turn_text(&context.text, context.config.plan_mode);
        // A `/name` skill gesture resolves against the agent this turn was
        // built with, exactly where the desktop runner resolves it: at the
        // send, not while the reader types. The transcript keeps the line the
        // reader wrote; only the request the model reads carries the skill's
        // instructions.
        let (content, _invoked) =
            crate::skill_invocation::expand(&built.agent, Content::Text(text));
        let mut answer = String::new();
        // Text that has arrived but not yet been reported as activity.
        let mut delta = String::new();
        // Calls the model asked for and whose result has not arrived yet.
        // The engine answers each call in order, but a parallel batch can
        // interleave names, so pairing is by name, not by position alone.
        let mut pending: VecDeque<(u64, String)> = VecDeque::new();
        let mut steps = 0u64;
        // Provider-reported usage, summed over the turn's model calls: a turn
        // that calls three tools is four requests, and the desktop's usage
        // pill counts the turn, not the last request.
        let mut usage: Option<TurnUsage> = None;
        // The working plan lives in the agent's own store, which the
        // `update_plan` tool writes through. Holding the same store here is
        // what lets a turn report the plan the moment the model replaces it,
        // so the Host journals the model's list and not a copy of a tool call.
        let plan = built.agent.plan();
        let mut stream = built.agent.stream(content);
        while let Some(item) = stream.next().await {
            match item {
                Ok(StreamToken::Text(text)) => {
                    answer.push_str(&text);
                    delta.push_str(&text);
                    if delta.chars().count() >= DELTA_CHARS {
                        context.progress.emit(TurnProgress::AssistantText {
                            text: std::mem::take(&mut delta),
                        });
                    }
                }
                // The engine keeps reasoning state in its own turn-local
                // session; the transcript does not persist or replay it.
                Ok(StreamToken::ReasoningContent(_) | StreamToken::ReasoningState(_)) => {}
                // Every model call of the turn reports its own usage block; the
                // turn's figure is their sum, which is what the desktop's
                // accumulator does with the same stream.
                Ok(StreamToken::Usage(reported)) => {
                    let bucket = usage.get_or_insert_with(TurnUsage::default);
                    bucket.prompt_tokens += u64::from(reported.prompt_tokens);
                    bucket.completion_tokens += u64::from(reported.completion_tokens);
                    bucket.cached_tokens += u64::from(
                        reported
                            .prompt_tokens_details
                            .as_ref()
                            .and_then(|details| details.cached_tokens)
                            .unwrap_or(0),
                    );
                }
                // The desktop transcript shows each call and its result; the
                // Host journals the pair so any client, not only the one
                // watching this turn, can render them.
                Ok(StreamToken::ToolCall { name, args, .. }) => {
                    steps += 1;
                    let summary = summarize_arguments(&args);
                    let arguments =
                        cap(&serde_json::to_string(&args).unwrap_or_default(), ARGS_CAP);
                    pending.push_back((steps, name.clone()));
                    context.progress.emit(TurnProgress::ToolCallStarted {
                        step: steps,
                        tool: name,
                        summary,
                        args: arguments,
                    });
                }
                Ok(StreamToken::ToolResult { name, output, ms }) => {
                    // A result without a matching call still belongs to a
                    // step of this turn: the engine may report a call the
                    // client never saw because the call itself was dropped.
                    let step = pending
                        .iter()
                        .position(|(_, tool)| tool == &name)
                        .map(|index| pending.remove(index).unwrap().0)
                        .unwrap_or_else(|| {
                            steps += 1;
                            steps
                        });
                    // A plan replacement is reported before the call's own
                    // result: the panel a client draws then already holds the
                    // steps the model just wrote, even while the call's
                    // verdict is still the last thing in the journal.
                    let is_plan = name == crate::runtime::PLAN_TOOL;
                    context.progress.emit(TurnProgress::ToolCallFinished {
                        step,
                        tool: name,
                        ok: tool_succeeded(&output),
                        output: cap(&output, OUTPUT_CAP),
                        duration_ms: ms,
                    });
                    if is_plan {
                        context.progress.emit(TurnProgress::PlanUpdated {
                            items: plan.items(),
                        });
                    }
                }
                Ok(StreamToken::Done | StreamToken::Stopped) => break,
                Err(error) => return Err(TurnError::new(error)),
            }
        }
        // The tail of an answer is usually shorter than one frame's worth; a
        // reader watching the turn still gets it before the commit.
        if !delta.is_empty() {
            context
                .progress
                .emit(TurnProgress::AssistantText { text: delta });
        }
        Ok(TurnOutput {
            text: answer,
            usage: usage.filter(|usage| !usage.is_empty()),
        })
    }
}

/// Seal a turn's changeset when the turn ends, however it ends.
///
/// A turn that failed half way still changed the files it touched: the desktop
/// seals at settle for the same reason, because a diff is part of what the turn
/// did and not part of whether it finished. Sealing nothing is a no-op, so the
/// guard also covers a turn that never wrote.
struct SealOnDrop(Arc<crate::review::store::ReviewRuntime>);

impl Drop for SealOnDrop {
    fn drop(&mut self) {
        let _ = self.0.seal();
    }
}

/// Whether one tool result reports success.
///
/// The engine hands the model a JSON envelope whose `status` is the verdict;
/// a result that is not that envelope is treated as text, where an `error`
/// prefix is the only failure marker the tools emit.
fn tool_succeeded(output: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(output) {
        Ok(value) => value
            .get("status")
            .and_then(|status| status.as_str())
            .is_none_or(|status| status != "error"),
        Err(_) => !output
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("error"),
    }
}

/// A one-line description of a tool call's arguments.
///
/// The keys below are the ones the shell, file and search tools lead with; a
/// call that names none of them falls back to its first string argument, and a
/// call with no text at all describes itself as empty rather than as `null`.
fn summarize_arguments(args: &serde_json::Value) -> String {
    const KEYS: [&str; 8] = [
        "command",
        "path",
        "file_path",
        "file",
        "pattern",
        "query",
        "url",
        "id",
    ];
    for key in KEYS {
        if let Some(text) = args.get(key).and_then(|value| value.as_str()) {
            return cap(text.trim(), SUMMARY_CAP);
        }
    }
    if let Some(text) = args
        .as_object()
        .and_then(|object| object.values().find_map(|value| value.as_str()))
    {
        return cap(text.trim(), SUMMARY_CAP);
    }
    String::new()
}

/// A string capped to `limit` characters, marked when it was cut.
fn cap(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The workspace root as one comparable path.
fn canonical_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// Whether a stored workspace root names the same directory as `root`.
///
/// The record is compared by canonical path, not by string: a settings file
/// written on another platform, or before a symlink resolved, still names the
/// same directory.
fn same_root(stored: &str, root: &Path) -> bool {
    let stored = PathBuf::from(stored);
    let stored = std::fs::canonicalize(&stored).unwrap_or(stored);
    stored == root
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::executor::HistoryMessage;
    use crate::runtime::host::ApprovalHandle;
    use crate::runtime::ids::{ExecutionId, SessionId, WorkspaceId};

    /// Settings that resolve a model, so a turn reaches the next gate.
    fn model_settings(permission: &str) -> Settings {
        let mut settings = Settings::default();
        settings
            .global
            .add_provider("test", "", "http://127.0.0.1:1/v1", &[]);
        let _ = settings
            .global
            .add_model("test", "fast", "", "test-model", "");
        settings.global.default_model = Some(crate::settings::global::ModelRef {
            provider: "test".to_string(),
            model: "fast".to_string(),
        });
        settings.workspace.agent.permission = permission.to_string();
        settings
    }

    /// A Host's runtime and the query a client reads are one store: the
    /// changeset a turn captures is the changeset `/review` lists, with the
    /// prepared diff a desktop Review tab draws.
    #[test]
    fn a_turn_review_is_the_changeset_the_review_query_reads() {
        use crate::review::domain::ChangeOperation;

        let executor = executor(model_settings("workspace-write"));
        let session = SessionId::new();
        let runtime = executor.start_review(session).expect("a review runtime");
        let root = executor.workspace_dir().expect("the scratch workspace");
        let path = root.join("notes.txt");
        std::fs::write(&path, "one\n").unwrap();
        let before = runtime.before(&path);
        std::fs::write(&path, "two\n").unwrap();
        runtime.record(&path, ChangeOperation::Write, before, "write_file");
        assert!(runtime.seal().is_some(), "the captured write seals");

        let report = executor.review_report(&session.to_string(), None);
        assert_eq!(report.state, "ready", "{:?}", report.error);
        assert_eq!(report.changes.len(), 1);
        assert_eq!(report.changes[0].path, "notes.txt");
        assert_eq!(report.changes[0].kind, "modified");
        assert_eq!(report.changes[0].tools, vec!["write_file".to_string()]);

        let named = executor.review_report(&session.to_string(), Some("notes.txt"));
        let diff = named.diff.expect("the prepared diff of the named change");
        assert_eq!(diff.path, "notes.txt");
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 1);
        assert_eq!(
            diff.lines
                .iter()
                .map(|line| (line.mark.as_str(), line.text.as_str()))
                .collect::<Vec<_>>(),
            vec![("-", "one"), ("+", "two")]
        );
    }

    /// `/diff` is the desktop Diff tab read through the Host: the list names
    /// every path the checkout changed with the letter the Files tree badges
    /// it with, and a path is that file's unified comparison. The VCS is the
    /// one the desktop asks, so the two can never disagree about what changed.
    #[test]
    fn the_diff_report_lists_the_changes_and_reads_one_patch() {
        fn git(root: &Path, args: &[&str]) {
            let status = std::process::Command::new("git")
                .current_dir(root)
                .args(args)
                .status()
                .expect("git runs");
            assert!(status.success(), "git {args:?} failed");
        }

        let root = scratch("diff-report");
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "bsh@example.com"]);
        git(&root, &["config", "user.name", "BSH"]);
        std::fs::write(root.join("tracked.txt"), "one\n").expect("write");
        git(&root, &["add", "tracked.txt"]);
        git(&root, &["commit", "-qm", "one"]);
        std::fs::write(root.join("tracked.txt"), "two\n").expect("modify");
        std::fs::write(root.join("added.txt"), "new\n").expect("add");

        let mut settings = Settings::default();
        let workspace = settings.note_workspace("scratch", &root.to_string_lossy());
        let executor = AgentExecutor::with_parts(
            settings,
            Vault::disabled(),
            workspace,
            Lang::En,
            crate::mcp::McpPool::default(),
        );

        // No path is the list: every changed path, sorted, with the shared
        // status vocabulary the Files tree uses.
        let list = executor.diff("");
        assert_eq!(list.state, "listed", "{:?}", list.error);
        assert_eq!(list.vcs, "git");
        let paths: Vec<&str> = list
            .changes
            .iter()
            .map(|change| change.path.as_str())
            .collect();
        assert_eq!(paths, ["added.txt", "tracked.txt"]);
        assert_eq!(list.changes[0].letter, "A");
        assert_eq!(list.changes[1].letter, "M");
        assert_eq!(list.changes[1].label, "Modified");

        // A path is its comparison, and the list travels beside it so a client
        // redrawing the tab after a diff keeps its rows.
        let patch = executor.diff("tracked.txt");
        assert_eq!(patch.state, "patch", "{:?}", patch.error);
        assert_eq!(patch.path, "tracked.txt");
        assert_eq!(patch.changes.len(), 2);
        let text = patch.patch.as_deref().unwrap_or_default();
        assert!(text.contains("-one"), "{text}");
        assert!(text.contains("+two"), "{text}");

        // The new file compares against nothing, so the VCS synthesises an
        // all-addition patch - the same answer the desktop pane draws.
        let added = executor.diff("added.txt");
        assert_eq!(added.state, "patch", "{:?}", added.error);
        assert!(added.patch.as_deref().unwrap_or_default().contains("+new"));

        // A tracked file that matches its commit is clean, not missing: there
        // is simply nothing that differs from the baseline.
        std::fs::write(root.join("tracked.txt"), "one\n").expect("restore");
        let clean = executor.diff("tracked.txt");
        assert_eq!(clean.state, "clean");
        assert!(clean.patch.is_none());

        // A path that climbs out is refused before the VCS is asked about a
        // file that is not the workspace's to compare.
        let outside = executor.diff("../../../etc/passwd");
        assert_eq!(outside.state, "refused");

        // A root that is no checkout has no baseline, so it says that instead
        // of claiming the working copy is clean.
        let bare = scratch("diff-not-a-checkout");
        let mut settings = Settings::default();
        let workspace = settings.note_workspace("scratch", &bare.to_string_lossy());
        let executor = AgentExecutor::with_parts(
            settings,
            Vault::disabled(),
            workspace,
            Lang::En,
            crate::mcp::McpPool::default(),
        );
        let unknown = executor.diff("");
        assert_eq!(unknown.state, "unknown");
        assert!(unknown.error.is_some(), "the refusal says why");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&bare);
    }

    /// A directory of this test's own, left behind for the next run to reuse.
    fn scratch(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("bsh-agent-exec-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("the scratch directory");
        root
    }

    /// The delegated children a turn spawns are the rows the query answers
    /// with: a terminal that opens the pane sees the run its conversation
    /// started, its state and its token use, and another conversation's run
    /// stays out of it.
    #[test]
    fn the_subagent_query_lists_the_runs_a_conversation_started() {
        let executor = executor(model_settings("workspace-write"));
        let mine = SessionId::new().to_string();
        let other = SessionId::new().to_string();
        let id = executor
            .subagents
            .try_begin(&mine, &mine, 1, 900, "Research", "look", 3)
            .expect("a free slot starts a run");
        executor
            .subagents
            .try_begin(&other, &other, 1, 900, "Unrelated", "look", 3)
            .expect("the second conversation gets its own slot");
        executor
            .subagents
            .finish(&id, "the answer".to_string(), (120, 30), Vec::new());

        // One conversation's lineage, which is what a client attached to a
        // chat asks for: its own run and nothing from the neighbouring chat.
        let report = executor.subagent_report(Some(mine.as_str()));
        assert_eq!(report.state, "ready");
        assert_eq!(report.runs.len(), 1, "only this conversation's run");
        let run = &report.runs[0];
        assert_eq!(run.id, id);
        assert_eq!(run.title, "Research");
        assert_eq!(run.status, "done");
        assert_eq!(run.prompt_tokens, 120);
        assert_eq!(run.completion_tokens, 30);
        assert_eq!(run.depth, 1);
        assert!(run.finished_at_ms.is_some());

        // The whole roster answers both chats, because a client that named no
        // conversation asked the Host what it holds.
        assert_eq!(executor.subagent_report(None).runs.len(), 2);

        // A conversation that delegated nothing is an empty roster, not the
        // unsupported answer a build without the tool gives: the registry
        // exists, so the count is a fact rather than a missing seam.
        let quiet = executor.subagent_report(Some(SessionId::new().to_string().as_str()));
        assert_eq!(quiet.state, "ready");
        assert!(quiet.runs.is_empty());
    }

    fn executor(mut settings: Settings) -> AgentExecutor {
        let root = scratch("workspace");
        let workspace = settings.note_workspace("scratch", &root.to_string_lossy());
        AgentExecutor::with_parts(
            settings,
            Vault::disabled(),
            workspace,
            Lang::En,
            crate::mcp::McpPool::default(),
        )
    }

    fn turn(text: &str) -> TurnContext {
        turn_with(text, SessionConfig::default())
    }

    /// A compaction of the history a Host projected.
    fn compact_context(history: Vec<HistoryMessage>) -> CompactContext {
        CompactContext {
            workspace_id: WorkspaceId::new(),
            session_id: SessionId::new(),
            config: SessionConfig::default(),
            history,
        }
    }

    /// A turn that runs with the session configuration a Host would hand it.
    fn turn_with(text: &str, config: SessionConfig) -> TurnContext {
        TurnContext {
            workspace_id: WorkspaceId::new(),
            session_id: SessionId::new(),
            execution_id: ExecutionId::new(),
            text: text.to_string(),
            history: Vec::new(),
            config,
            approvals: ApprovalHandle::detached(),
            progress: crate::runtime::executor::ProgressHandle::detached(),
        }
    }

    #[tokio::test]
    async fn a_turn_with_no_model_names_the_missing_model() {
        let error = executor(Settings::default())
            .execute(turn("hello"))
            .await
            .expect_err("a turn cannot run without a model");
        assert!(
            error.message.contains("no model resolved"),
            "the refusal says what is missing: {}",
            error.message
        );
    }

    /// The model check runs before the approval gate: an operator is never
    /// asked to authorize a turn that cannot start.
    #[tokio::test]
    async fn the_model_gate_precedes_the_approval_gate() {
        let mut settings = Settings::default();
        settings.workspace.agent.permission = crate::caps::FULL_ACCESS.to_string();
        let config = SessionConfig {
            permission: crate::caps::FULL_ACCESS.to_string(),
            ..SessionConfig::default()
        };
        let error = executor(settings)
            .execute(turn_with("hello", config))
            .await
            .expect_err("a turn cannot run without a model");
        assert!(
            error.message.contains("no model resolved"),
            "the model is named before the operator is asked: {}",
            error.message
        );
        assert!(!error.message.contains("approval"));
    }

    /// An unconfined turn asks the operator first. With no operator attached
    /// (the detached gate of a bare executor) it refuses instead of running the
    /// model's commands unbounded.
    #[tokio::test]
    async fn an_unconfined_turn_is_refused_without_an_operator() {
        let settings = model_settings(crate::caps::FULL_ACCESS);
        let config = SessionConfig {
            permission: crate::caps::FULL_ACCESS.to_string(),
            ..SessionConfig::default()
        };
        let error = executor(settings)
            .execute(turn_with("rm -rf .", config))
            .await
            .expect_err("an unconfined turn needs a decision");
        assert!(
            error.message.contains("needs the operator's approval"),
            "the refusal names the missing decision: {}",
            error.message
        );
    }

    /// The confined default asks nothing: a workspace-write turn reaches the
    /// provider instead of stopping at the gate. The provider is a closed local
    /// port, so the turn fails at the wire and never leaves the machine.
    #[tokio::test]
    async fn a_confined_turn_does_not_ask_the_operator() {
        let settings = model_settings(crate::caps::WORKSPACE_WRITE);
        let error = executor(settings)
            .execute(turn("hello"))
            .await
            .expect_err("the closed provider port fails the turn");
        assert!(
            !error.message.contains("needs the operator's approval"),
            "a confined turn is not gated: {}",
            error.message
        );
    }

    /// The Host asks the executor what a new session starts with, so the
    /// answer has to be a resolution of this machine's settings rather than a
    /// contract constant.
    #[test]
    fn the_default_configuration_carries_the_resolved_permission() {
        let mut settings = model_settings(crate::caps::READ_ONLY);
        settings.workspace.agent.permission = crate::caps::READ_ONLY.to_string();
        let executor = executor(settings);
        let config = executor.default_config();
        assert_eq!(config.permission, crate::caps::READ_ONLY);
        assert_eq!(
            config.model_ref(),
            None,
            "a fresh session follows the default model"
        );
        assert_eq!(config.preset, None);
        assert!(!config.plan_mode);
    }

    /// The `/settings` pane reads the Host's own resolution: the rail the
    /// desktop dialog edits, the effective value of each row and where it came
    /// from. A named section narrows to one cell, case-insensitively, because
    /// the client folds the name the same way; a name the rail lacks answers
    /// with the cells it has instead of an empty pane.
    #[test]
    fn the_settings_query_answers_the_rail_the_terminal_lists() {
        let settings = model_settings(crate::caps::READ_ONLY);
        let executor = executor(settings);
        // Qualified: `AgentExecutor` also has an inherent `settings()` accessor
        // that returns its lock guard, and that one would win the method call.
        let report = TurnExecutor::settings(&executor, None);
        assert_eq!(report.state, "ready", "a built executor resolves settings");
        assert!(report.error.is_none());
        assert!(
            report
                .sections
                .iter()
                .any(|section| section.section == "model"),
            "the rail starts with the model cell: {:?}",
            report
                .sections
                .iter()
                .map(|section| section.section.as_str())
                .collect::<Vec<_>>()
        );
        // Provenance is a closed set of words, and a credential row never
        // carries the credential itself.
        for section in &report.sections {
            for row in &section.rows {
                assert!(
                    [
                        "settings",
                        "workspace",
                        "vault",
                        "env",
                        "default",
                        "unset",
                        "preset"
                    ]
                    .contains(&row.source.as_str()),
                    "row {} names an unknown provenance: {}",
                    row.label,
                    row.source
                );
            }
        }
        // Every key a write accepts is a row the rail shows: a field the reader
        // could set but never see would be a setting only a manual could find.
        let keys: Vec<&str> = report
            .sections
            .iter()
            .flat_map(|section| section.rows.iter())
            .filter_map(|row| row.key.as_deref())
            .collect();
        for key in crate::settings::edit::EDITABLE_KEYS {
            assert!(
                keys.contains(&key),
                "the rail lists no '{key}' row, so no reader would find it"
            );
        }
        // The rail's cells are the desktop dialog's, so a reader who knows the
        // window's names can type the same name here.
        for name in [
            "model",
            "workspace",
            "tools",
            "agent-loop",
            "web-search",
            "vault",
            "general",
        ] {
            assert!(
                report
                    .sections
                    .iter()
                    .any(|section| section.section == name),
                "the rail is missing the '{name}' cell"
            );
        }
        let model = report
            .sections
            .iter()
            .find(|section| section.section == "model")
            .expect("the model cell");
        let credential = model
            .rows
            .iter()
            .find(|row| row.label == "credential")
            .expect("a credential row");
        assert!(
            !credential.value.contains("sk-") && credential.value.len() < 80,
            "a credential row names where the key lives, never the key"
        );

        let narrowed = TurnExecutor::settings(&executor, Some("MODEL"));
        assert_eq!(narrowed.sections.len(), 1);
        assert_eq!(narrowed.sections[0].section, "model");
        assert!(narrowed.error.is_none());

        let unknown = TurnExecutor::settings(&executor, Some("theme"));
        assert!(unknown.sections.is_empty());
        let reason = unknown.error.expect("the rail refuses an unknown cell");
        assert!(reason.contains("known:"), "{reason}");
    }

    /// The write side of the same query: a value the Settings dialog could
    /// write lands in BOTH documents the next turn reads, and a value the
    /// dialog would refuse changes neither the file nor the live resolution.
    ///
    /// The store is redirected to a scratch directory because a test build that
    /// wrote the reader's settings file would be a defect, not a test - the
    /// guard in `save_for_root` is what makes that impossible even when a test
    /// forgets the redirect, and this test proves the redirect is enough.
    #[test]
    fn a_settings_write_lands_in_the_documents_the_next_turn_reads() {
        let store = scratch("settings-store");
        let mut settings = model_settings(crate::caps::READ_ONLY);
        settings.save_root = Some(store.clone());
        let executor = executor(settings);
        let root = executor.workspace_dir().expect("the scratch workspace");

        let report = executor
            .update_settings(&[
                SettingEdit {
                    key: "temperature".to_string(),
                    value: "0.4".to_string(),
                },
                // Grouping and a suffix are the dialog's own spellings, so a
                // terminal that types them writes the count the dialog writes.
                SettingEdit {
                    key: "max-tokens".to_string(),
                    value: "8K".to_string(),
                },
                // A machine-wide field, so the global document is exercised too.
                SettingEdit {
                    key: "web-search-results".to_string(),
                    value: "12".to_string(),
                },
            ])
            .expect("each value is one the desktop's form would store");

        // The answer is the resolution the NEXT turn uses, and every row the
        // write addressed carries its stored value rather than the typed text.
        assert_eq!(report.state, "ready");
        let row = |key: &str| {
            report
                .sections
                .iter()
                .flat_map(|section| section.rows.iter())
                .find(|row| row.key.as_deref() == Some(key))
                .unwrap_or_else(|| panic!("the report lists no '{key}' row"))
                .value
                .clone()
        };
        assert_eq!(row("temperature"), "0.4");
        assert_eq!(row("max-tokens"), "8192", "8K is written as its count");
        assert_eq!(row("web-search-results"), "12");

        // The workspace document is the one under the root this Host serves,
        // not whichever project the reader's window opened last.
        let workspace_document = crate::settings::workspace::WorkspaceSettings::path(&root);
        let workspace = std::fs::read_to_string(&workspace_document)
            .unwrap_or_else(|error| panic!("the workspace document: {error}"));
        assert!(workspace.contains("0.4"), "{workspace}");
        assert!(workspace.contains("8192"), "{workspace}");
        // The global document is written where the redirect points.
        let global = std::fs::read_to_string(store.join("settings.toml"))
            .expect("the global document beside the redirect");
        assert!(global.contains("results_per_search"), "{global}");
        assert!(global.contains("12"), "{global}");

        // The live resolution moved with the documents: a query answers the new
        // value without re-reading from disk.
        let live = executor.settings();
        assert_eq!(live.workspace.agent.temperature, Some(0.4));
        assert_eq!(live.workspace.agent.max_tokens, Some(8192));
        assert_eq!(live.global.web_search.results_per_search, 12);
        drop(live);

        // A refused value is not stored and does not reach the live resolution,
        // and the reader is told which rule it broke.
        let before = workspace.clone();
        let error = executor
            .update_settings(&[SettingEdit {
                key: "temperature".to_string(),
                value: "3".to_string(),
            }])
            .expect_err("3 is not a sampling temperature");
        assert!(error.contains("0 to 2"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&workspace_document).expect("the document"),
            before,
            "a refused write leaves the document byte-identical"
        );
        assert_eq!(
            executor.settings().workspace.agent.temperature,
            Some(0.4),
            "the live resolution keeps the value that landed"
        );

        // A key this Host does not accept, and a write that names nothing, are
        // refused rather than treated as no-ops.
        assert!(
            executor
                .update_settings(&[SettingEdit {
                    key: "theme".to_string(),
                    value: "dark".to_string(),
                }])
                .is_err(),
            "an invented key is refused"
        );
        assert!(executor.update_settings(&[]).is_err(), "an empty patch");
    }

    /// The catalogue advertises the skills this build registered, name-ordered,
    /// so a terminal offers the same `/name` rows the desktop palette does.
    #[test]
    fn the_catalogue_lists_the_registered_skills() {
        let dir = scratch("skills");
        for (name, description) in [("release", "ship the release"), ("audit", "read the log")] {
            let skill = dir.join(name);
            std::fs::create_dir_all(&skill).expect("the skill directory");
            std::fs::write(
                skill.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {description}\n---\n\nBody.\n"),
            )
            .expect("the skill manifest");
        }
        let mut settings = Settings::default();
        settings
            .global
            .set_skill_sources(vec![crate::settings::global::SkillSource {
                name: "Test".to_string(),
                path: dir.to_string_lossy().to_string(),
                enabled: true,
            }]);
        let catalogue = executor(settings).catalogue();
        let names: Vec<&str> = catalogue
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        assert_eq!(names, ["audit", "release"], "the rows are name-ordered");
        assert_eq!(catalogue.skills[1].description, "ship the release");
        assert!(!catalogue.skills[1].user_only);
    }

    /// A model, preset or permission reaches the session only when the turn
    /// that will run could actually use it; the catalogue is the authority.
    #[test]
    fn a_configuration_patch_is_validated_against_the_catalogue() {
        let executor = executor(model_settings(crate::caps::WORKSPACE_WRITE));
        let current = SessionConfig::default();
        let pair = |provider: &str, model: &str| SessionConfigChange {
            provider: Some(provider.to_string()),
            model: Some(model.to_string()),
            ..SessionConfigChange::default()
        };
        let next = executor
            .configure(&current, &pair("test", "fast"))
            .expect("the configured model is accepted");
        assert_eq!(
            next.model_ref(),
            Some(("test".to_string(), "fast".to_string()))
        );
        // An unknown model is refused instead of being stored for the next
        // turn to fail on at the provider.
        let unknown = executor
            .configure(&current, &pair("test", "absent"))
            .expect_err("an unknown model is refused");
        assert!(unknown.contains("no model 'absent'"), "{unknown}");
        // Half a pair names no model at all.
        let half = SessionConfigChange {
            provider: Some("test".to_string()),
            ..SessionConfigChange::default()
        };
        assert!(executor.configure(&current, &half).is_err());
        // A permission outside the contract's vocabulary is refused before
        // any catalogue lookup.
        let bad = SessionConfigChange {
            permission: Some("yolo".to_string()),
            ..SessionConfigChange::default()
        };
        assert!(
            executor
                .configure(&current, &bad)
                .expect_err("an unknown mode is refused")
                .contains("unknown permission")
        );
        // A preset must be one the workspace is configured with.
        let preset = SessionConfigChange {
            preset: Some("not-a-preset".to_string()),
            ..SessionConfigChange::default()
        };
        assert!(
            executor
                .configure(&current, &preset)
                .expect_err("an unknown preset is refused")
                .contains("no preset")
        );
    }

    /// Compaction is a model call, so an executor with no model refuses before
    /// any request: the operator hears which piece is missing.
    #[tokio::test]
    async fn compaction_without_a_model_names_the_missing_model() {
        let error = executor(Settings::default())
            .compact(compact_context(Vec::new()))
            .await
            .expect_err("no model means no summarizer");
        assert!(
            error.message.contains("no model resolved"),
            "the refusal says what is missing: {}",
            error.message
        );
    }

    /// A session with nothing safe to fold refuses instead of installing a
    /// checkpoint over rows that still exist.
    #[tokio::test]
    async fn an_empty_history_refuses_to_compact() {
        let error = executor(model_settings(crate::caps::WORKSPACE_WRITE))
            .compact(compact_context(Vec::new()))
            .await
            .expect_err("an empty history has nothing to fold");
        assert!(
            error.message.contains("nothing in this session's history"),
            "the refusal names the empty history: {}",
            error.message
        );
    }

    #[test]
    fn a_stored_root_is_matched_by_its_canonical_path() {
        let directory = scratch("same-root");
        let root = canonical_root(&directory);
        assert!(same_root(&directory.to_string_lossy(), &root));
        assert!(!same_root("/definitely/not/this/directory", &root));
    }

    /// The Host's roster is the settings document made live: an enabled server
    /// that cannot start is listed with its reason, a switched-off one is listed
    /// as off and never connected, and neither can fail the Host.
    #[test]
    fn configured_servers_reach_the_pool_and_the_roster() {
        let mut settings = Settings::default();
        settings.global.mcp_servers = vec![
            serde_json::json!({
                "name": "missing",
                "command": "bsh-mcp-server-that-does-not-exist",
            }),
            serde_json::json!({
                "name": "off",
                "command": "/bin/true",
                "enabled": false,
            }),
        ];
        let root = scratch("mcp-roster");
        let workspace = settings.note_workspace("scratch", &root.to_string_lossy());
        let choice = SessionChoice {
            workspace: Some(workspace),
            ..SessionChoice::default()
        };
        let (pool, failed) = connect_mcp(&settings, &choice);
        assert_eq!(
            failed,
            ["missing"],
            "a server that cannot start is reported, not fatal"
        );
        let roster = mcp_roster(&settings, &pool);
        let names: Vec<&str> = roster.iter().map(|server| server.name.as_str()).collect();
        assert_eq!(names, ["missing", "off"], "name-ordered, disabled included");
        let missing = &roster[0];
        assert!(missing.enabled);
        assert!(
            missing.error.is_some(),
            "the pool's failure reason reaches the client"
        );
        assert!(missing.tools.is_empty());
        assert_eq!(missing.transport, "bsh-mcp-server-that-does-not-exist");
        let off = &roster[1];
        assert!(!off.enabled);
        assert!(
            off.error.is_none(),
            "a switched-off server is not a failure"
        );
        assert!(off.tools.is_empty());
    }

    /// A configured stdio server really connects: the roster states the exact
    /// tool names, and the agent this Host builds registers them for the model.
    #[test]
    fn a_connected_server_lists_the_tools_the_turn_can_call() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent/tests/fixtures/mock_mcp_server.py");
        let mut settings = model_settings(crate::caps::WORKSPACE_WRITE);
        settings.global.mcp_servers = vec![serde_json::json!({
            "name": "fixture",
            "command": "python3",
            "args": [fixture.to_string_lossy()],
        })];
        let root = scratch("mcp-connected");
        let workspace = settings.note_workspace("scratch", &root.to_string_lossy());
        let choice = SessionChoice {
            workspace: Some(workspace.clone()),
            ..SessionChoice::default()
        };
        let (pool, failed) = connect_mcp(&settings, &choice);
        assert!(failed.is_empty(), "the fixture server connects: {failed:?}");
        let roster = mcp_roster(&settings, &pool);
        assert_eq!(roster.len(), 1);
        let server = &roster[0];
        assert!(server.error.is_none(), "a connected server has no reason");
        assert!(
            server.transport.starts_with("python3 "),
            "the launch line travels"
        );
        assert_eq!(
            server.tools,
            ["fixture_echo_tool", "fixture_echo_tool_upper"],
            "the roster names the tools the model calls"
        );

        // The pool is what the turn hands the agent build, so the names the
        // reader sees are the names in the model's tool list.
        let executor =
            AgentExecutor::with_parts(settings, Vault::disabled(), workspace, Lang::En, pool);
        let built = executor.build(&SessionConfig::default(), SessionId::new(), None);
        let names = built
            .agent
            .registry()
            .map(|registry| registry.list())
            .unwrap_or_default();
        assert!(
            names.iter().any(|name| name == "fixture_echo_tool"),
            "the MCP tool reached the built agent: {names:?}"
        );
    }

    /// `/tasks` reads the store the desktop board and the scheduler share: the
    /// record a task tool wrote lands in the section its status belongs to, with
    /// the record's own spellings and the next fire the trigger knows.
    #[test]
    fn the_task_board_lists_the_workspace_records() {
        use crate::extension::tasks::{
            FileTasksStore, Origin, Task, TasksStore as _, Tier, Trigger,
        };

        let root = scratch("tasks-board");
        let now_ms = 1_700_000_000_000;
        let queued = Task::open(
            "task-queued",
            "ship the release",
            "cut the release notes",
            Origin {
                session: "session-1".to_string(),
                turn: 2,
            },
            Trigger::Now,
            Tier::Turn,
            now_ms,
        )
        .expect("a turn task may fire now");
        FileTasksStore
            .save(&root, &queued)
            .expect("the queued record");
        let scheduled = Task::open(
            "task-scheduled",
            "refresh the index",
            "rebuild the search index",
            Origin {
                session: "session-2".to_string(),
                turn: 0,
            },
            Trigger::Every {
                every_seconds: 3_600,
                last_run_ms: 0,
            },
            Tier::Job,
            now_ms + 1_000,
        )
        .expect("a job may run on a cadence");
        FileTasksStore
            .save(&root, &scheduled)
            .expect("the scheduled record");
        // A delegated child names the task it hangs under and the task it waits
        // on: the two edges the board's graph view draws.
        let mut child = Task::open(
            "task-child",
            "cut the release notes",
            "cut the notes the release points at",
            Origin {
                session: "session-1".to_string(),
                turn: 3,
            },
            Trigger::Now,
            Tier::Subagent,
            now_ms + 2_000,
        )
        .expect("an immediate child task is legal");
        child.parent = Some("task-queued".to_string());
        child.deps = vec!["task-queued".to_string()];
        FileTasksStore
            .save(&root, &child)
            .expect("the child record");

        let mut settings = Settings::default();
        let workspace = settings.note_workspace("scratch", &root.to_string_lossy());
        let executor = AgentExecutor::with_parts(
            settings,
            Vault::disabled(),
            workspace,
            Lang::En,
            crate::mcp::McpPool::default(),
        );
        let report = executor.tasks();
        assert_eq!(report.state, "ready");
        assert_eq!(
            report.root,
            crate::caps::expand_path(&root.to_string_lossy()).to_string_lossy()
        );
        let sections: Vec<&str> = report
            .sections
            .iter()
            .map(|section| section.section.as_str())
            .collect();
        assert_eq!(
            sections,
            ["running", "queued", "scheduled", "history"],
            "the board travels in the desktop's own section order"
        );
        assert!(report.sections[0].rows.is_empty());
        let queued_row = &report.sections[1].rows[0];
        assert_eq!(queued_row.title, "ship the release");
        assert_eq!(queued_row.status, "queued");
        assert_eq!(queued_row.tier, "turn");
        assert_eq!(queued_row.host, "local");
        assert_eq!(queued_row.session, "session-1");
        assert_eq!(queued_row.attempts, 0);
        assert_eq!(queued_row.verification, "not_requested");
        assert!(
            queued_row.parent.is_empty() && queued_row.deps.is_empty(),
            "a task that names no edge carries none"
        );
        // The child travels with the edges the graph needs, so a client can nest
        // it under its parent without asking for the record.
        let child_row = &report.sections[1].rows[1];
        assert_eq!(child_row.parent, "task-queued");
        assert_eq!(child_row.deps, vec!["task-queued".to_string()]);
        let scheduled_row = &report.sections[2].rows[0];
        assert_eq!(scheduled_row.status, "scheduled");
        assert_eq!(scheduled_row.tier, "job");
        assert!(
            scheduled_row.next_fire_ms.is_some(),
            "a cadence names when it fires next"
        );
        assert!(report.sections[3].rows.is_empty());
    }

    /// `/tasks <id>` opens the record the board row summarised: the objective,
    /// the edges the graph draws, the caps each attempt runs under and what
    /// every attempt returned. The answer comes from the same store the board
    /// is read from, so a row the board lists never opens as missing.
    #[test]
    fn the_task_detail_answers_the_record_behind_a_board_row() {
        use crate::extension::tasks::{
            Attempt, AttemptStatus, FileTasksStore, Origin, Task, TasksStore as _, Tier, Trigger,
            VerificationContract, VerificationStatus,
        };

        let root = scratch("tasks-detail");
        let mut task = Task::open(
            "task-release",
            "ship the release",
            "cut the release branch and publish",
            Origin {
                session: "session-1".to_string(),
                turn: 2,
            },
            Trigger::Every {
                every_seconds: 600,
                last_run_ms: 1_700_000_000_000,
            },
            Tier::Turn,
            1_700_000_000_000,
        )
        .expect("a turn task may fire now");
        task.deps = vec!["build".to_string(), "lint".to_string()];
        task.deps.push("gate".to_string());
        let mut first = Attempt::queued(format!("{}-1", task.id), Tier::Turn);
        first.status = AttemptStatus::Succeeded;
        first.result = Some("pushed the tag".to_string());
        first.change_set_id = Some("cs-17".to_string());
        let mut second = Attempt::queued(format!("{}-2", task.id), Tier::Turn);
        second.status = AttemptStatus::Failed;
        second.error = Some("the suite timed out".to_string());
        task.attempts = vec![first, second];
        task.verification.status = VerificationStatus::Pending;
        task.verification_contract = Some(VerificationContract::Review);
        FileTasksStore.save(&root, &task).expect("the record");

        let mut settings = Settings::default();
        let workspace = settings.note_workspace("scratch", &root.to_string_lossy());
        let executor = AgentExecutor::with_parts(
            settings,
            Vault::disabled(),
            workspace,
            Lang::En,
            crate::mcp::McpPool::default(),
        );
        let report = executor.task_detail("task-release");
        assert_eq!(report.state, "ready");
        assert_eq!(
            report.root,
            crate::caps::expand_path(&root.to_string_lossy()).to_string_lossy()
        );
        assert_eq!(report.id, "task-release");
        assert_eq!(report.title, "ship the release");
        assert_eq!(report.intent, "cut the release branch and publish");
        assert_eq!(report.tier, "turn");
        assert_eq!(report.host, "local");
        assert_eq!(report.trigger, "every 600s");
        assert!(
            report.next_fire_ms.is_some(),
            "a cadence names its next fire"
        );
        assert_eq!(report.deps, ["build", "lint", "gate"]);
        assert_eq!(report.max_steps, task.budget.max_steps);
        assert_eq!(report.max_tokens, task.budget.max_tokens);
        assert_eq!(report.deadline_ms, task.budget.deadline_ms);
        assert_eq!(report.retry, "manual");
        assert_eq!(report.max_attempts, task.retry.max_attempts);
        assert_eq!(report.verification, "pending");
        assert_eq!(report.verification_contract, "review");
        assert_eq!(report.attempts.len(), 2);
        assert_eq!(report.attempts[0].id, "task-release-1");
        assert_eq!(report.attempts[0].status, "succeeded");
        assert_eq!(report.attempts[0].result, "pushed the tag");
        assert_eq!(report.attempts[0].change_set, "cs-17");
        assert_eq!(report.attempts[1].status, "failed");
        assert_eq!(report.attempts[1].error, "the suite timed out");

        // An id no record carries, and one a path cannot name, are both
        // `unknown`: the store was asked and there is nothing to answer with.
        let missing = executor.task_detail("ghost");
        assert_eq!(missing.state, "unknown");
        assert!(
            missing
                .error
                .as_deref()
                .is_some_and(|why| why.contains("ghost")),
            "the id that was asked for travels: {:?}",
            missing.error
        );
        let traversal = executor.task_detail("../escape");
        assert_eq!(traversal.state, "unknown");
        assert!(traversal.error.is_some());
    }

    /// `/memory` reads the store a turn recalls from: the record a workspace
    /// committed is listed with its facts, and a search returns it as a scored
    /// hit with the kernel's explanation.
    #[tokio::test]
    async fn the_memory_report_lists_and_recalls_the_workspace_store() {
        use agent::memory::MemoryRepository as _;

        let root = scratch("memory");
        let mut settings = Settings::default();
        let workspace = settings.note_workspace("scratch", &root.to_string_lossy());
        let binding = crate::memory_scope::binding_for(&settings, Some(&root));
        let store = agent::memory::FileMemory::open_scoped(
            &binding.workspace_store,
            binding.storage_scope.clone(),
            Some(binding.storage_scope.clone()),
        )
        .await
        .expect("the workspace store");
        let record = agent::memory::MemoryRecord::new(
            agent::memory::MemoryKind::Fact,
            binding.storage_scope.clone(),
            "the release checklist lives in docs/release.md",
            agent::memory::Provenance::default(),
        );
        store
            .repository()
            .add_record(record)
            .await
            .expect("a committed record");

        let executor = AgentExecutor::with_parts(
            settings,
            Vault::disabled(),
            workspace,
            Lang::En,
            crate::mcp::McpPool::default(),
        );
        let report = executor.memories(None);
        assert_eq!(report.state, "ready");
        assert_eq!(report.memories.len(), 1);
        assert_eq!(report.memories[0].kind, "fact");
        assert_eq!(report.memories[0].scope, "workspace");
        assert_eq!(report.memories[0].lifecycle, "active");

        let recall = executor.memories(Some("release checklist"));
        assert_eq!(recall.state, "ready");
        assert!(
            recall
                .memories
                .iter()
                .any(|view| view.score.is_some() && view.content.contains("release checklist")),
            "a search returns the record with its score: {:?}",
            recall.memories
        );
    }

    /// `/browser` reads the page the agent's browser tools left behind: the
    /// executor reports its own handle, so the pane, the tools and the model
    /// can never hold three opinions about where the browser is.
    #[test]
    fn the_browser_report_is_the_page_the_tools_read() {
        let executor = executor(model_settings("workspace-write"));
        let next = std::sync::Arc::new(std::sync::Mutex::new(1u64));
        executor.browser.mutate(|browser| {
            let mut next_id = next.lock().expect("the element counter");
            let document = crate::browser::parse_document(
                "https://example.com/docs",
                "<html><head><title>Docs</title></head><body>
                 <h1>Read the release checklist.</h1>
                 <a href=\"https://example.com/changelog\">Changelog</a>
                 <input name=\"q\" placeholder=\"Search\"></body></html>",
                &mut next_id,
            );
            browser.adopt(document, None);
        });

        let view = executor.browser();
        assert_eq!(view.status, "ready", "a landed page is ready");
        assert_eq!(view.url, "https://example.com/docs");
        assert_eq!(view.title, "Docs");
        assert!(
            view.text.contains("Read the release checklist."),
            "{:?}",
            view.text
        );
        assert_eq!(view.elements_total, 2, "the link and the field are listed");
        let link = view
            .elements
            .iter()
            .find(|element| element.kind == "link")
            .expect("the link");
        assert_eq!(link.label, "Changelog");
        assert_eq!(link.href.as_deref(), Some("https://example.com/changelog"));
        let field = view
            .elements
            .iter()
            .find(|element| element.kind == "input")
            .expect("the search field");
        assert_eq!(field.input_type.as_deref(), Some("text"));
        // The browser is not fetched by a read: opening the pane reports the
        // page the tools left and starts no request of its own.
        assert!(view.error.is_none(), "{:?}", view.error);
    }
}
