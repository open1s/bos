//! The typed host seam extensions use to start and observe work: host.runs.
//!
//! The seam is deliberately narrow-only. An extension may say WHAT it wants
//! (tier, objective, session, caps, trace) and never HOW it runs (no model, no
//! preset, no workspace, no permission tier). The host keeps those, which is
//! what makes an automation grant defensible.
//!
//! A host service is installed by the shell once it can serve runs. Until then
//! a call is refused with a stable message; the shell never fakes a run.

use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// The execution tier a run binds to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunTier {
    /// A turn in the named session.
    Turn,
    /// A child run inside a turn.
    Subagent,
    /// A background job.
    Job,
    /// A delegation to a peer node.
    Delegation,
}

/// The caps for one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Limits {
    /// Maximum steps.
    pub(crate) steps: u32,
    /// Maximum model tokens.
    pub(crate) tokens: u64,
    /// Maximum wall clock, in milliseconds.
    pub(crate) deadline_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            steps: 20,
            tokens: 200_000,
            deadline_ms: 600_000,
        }
    }
}

/// What an extension may ask for. There is no model, preset, workspace or
/// permission field on purpose: the host owns those.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunSpec {
    /// The session the run belongs to.
    pub(crate) session: String,
    /// The objective, prompt or command.
    pub(crate) objective: String,
    /// The tier.
    pub(crate) tier: RunTier,
    /// The caps.
    #[serde(default)]
    pub(crate) limits: Limits,
    /// For a delegation: the peer node id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<String>,
    /// For a delegation: an optional capability allowlist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) capability: Option<String>,
    /// The origin task or run this belongs to, for audit and the board.
    pub(crate) trace: String,
}

impl RunSpec {
    /// Refuse a spec the seam cannot honour, naming the field.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.session.trim().is_empty() {
            return Err("a run needs a session".to_string());
        }
        if self.objective.trim().is_empty() {
            return Err("a run needs an objective".to_string());
        }
        if self.trace.trim().is_empty() {
            return Err("a run needs a trace id".to_string());
        }
        match self.tier {
            RunTier::Delegation => {
                if self
                    .target
                    .as_deref()
                    .map(str::trim)
                    .filter(|target| !target.is_empty())
                    .is_none()
                {
                    return Err("a delegation needs a target node".to_string());
                }
            }
            _ => {
                if self.target.is_some() {
                    return Err("only a delegation names a target node".to_string());
                }
            }
        }
        Ok(())
    }
}

/// The opaque id of a started run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunHandle {
    /// Stable id.
    pub(crate) id: String,
}

/// The observed state of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunState {
    /// Accepted but not started.
    Queued,
    /// Live.
    Running,
    /// Finished successfully.
    Succeeded,
    /// Finished with an error.
    Failed,
    /// Stopped deliberately.
    Cancelled,
    /// The peer could not be reached.
    Unknown,
}

impl RunState {
    /// Whether the run will not change again.
    pub(crate) fn is_settled(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// What observing a run reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunSnapshot {
    /// The run id.
    pub(crate) id: String,
    /// Its state.
    pub(crate) state: RunState,
    /// The bounded result text, once settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<String>,
    /// Tokens used.
    #[serde(default)]
    pub(crate) tokens: u64,
    /// Steps taken.
    #[serde(default)]
    pub(crate) steps: u32,
    /// The error or refusal reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

/// What a host implementation must provide.
pub(crate) trait RunsService: Send + Sync {
    /// Start a run.
    fn start(&self, spec: RunSpec) -> Result<RunHandle, String>;
    /// Observe a run; None when the id is unknown.
    fn observe(&self, id: &str) -> Option<RunSnapshot>;
    /// Stop a run.
    fn cancel(&self, id: &str) -> Result<(), String>;
}

/// The refusal every call reports while no host service is installed.
pub(crate) const NO_SERVICE: &str = "no host.runs service is installed";

/// A handle to the host's runs, or none.
#[derive(Clone, Default)]
pub(crate) struct Runs {
    service: Option<Arc<dyn RunsService>>,
}

impl Runs {
    /// An uninstalled seam, which refuses.
    pub(crate) fn empty() -> Self {
        Self { service: None }
    }

    /// A seam over one service.
    pub(crate) fn with(service: Arc<dyn RunsService>) -> Self {
        Self {
            service: Some(service),
        }
    }

    /// Whether a service is installed.
    pub(crate) fn is_installed(&self) -> bool {
        self.service.is_some()
    }

    /// Start a validated run.
    pub(crate) fn start(&self, spec: RunSpec) -> Result<RunHandle, String> {
        spec.validate()?;
        self.service
            .as_ref()
            .ok_or_else(|| NO_SERVICE.to_string())?
            .start(spec)
    }

    /// Observe a run, or nothing while uninstalled.
    pub(crate) fn observe(&self, id: &str) -> Option<RunSnapshot> {
        self.service.as_ref()?.observe(id)
    }

    /// Stop a run.
    pub(crate) fn cancel(&self, id: &str) -> Result<(), String> {
        self.service
            .as_ref()
            .ok_or_else(|| NO_SERVICE.to_string())?
            .cancel(id)
    }
}

static INSTALLED: OnceLock<Mutex<Option<Arc<dyn RunsService>>>> = OnceLock::new();

fn installed() -> &'static Mutex<Option<Arc<dyn RunsService>>> {
    INSTALLED.get_or_init(|| Mutex::new(None))
}

/// Install the host's service. Idempotent: a second install replaces it.
pub(crate) fn install_runs_service(service: Arc<dyn RunsService>) {
    if let Ok(mut slot) = installed().lock() {
        *slot = Some(service);
    }
}

/// Remove the host's service, so later calls refuse.
pub(crate) fn clear_runs_service() {
    if let Ok(mut slot) = installed().lock() {
        *slot = None;
    }
}

/// Remove the host's service only when it is this exact one, so dropping one
/// window cannot withdraw a service another window installed.
pub(crate) fn clear_runs_service_if(service: &Arc<dyn RunsService>) {
    if let Ok(mut slot) = installed().lock() {
        if slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, service))
        {
            *slot = None;
        }
    }
}

/// The process-wide service, if any.
pub(crate) fn runs() -> Runs {
    let service = installed().lock().ok().and_then(|slot| slot.clone());
    match service {
        Some(service) => Runs::with(service),
        None => Runs::empty(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Default)]
    struct Fake {
        specs: Mutex<Vec<RunSpec>>,
        runs: Mutex<BTreeMap<String, RunSnapshot>>,
        started: Mutex<u32>,
    }

    impl RunsService for Fake {
        fn start(&self, spec: RunSpec) -> Result<RunHandle, String> {
            let mut started = self.started.lock().expect("started");
            *started += 1;
            let id = format!("run-{started}");
            self.specs.lock().expect("specs").push(spec);
            self.runs.lock().expect("runs").insert(
                id.clone(),
                RunSnapshot {
                    id: id.clone(),
                    state: RunState::Running,
                    result: None,
                    tokens: 0,
                    steps: 0,
                    error: None,
                },
            );
            Ok(RunHandle { id })
        }

        fn observe(&self, id: &str) -> Option<RunSnapshot> {
            self.runs.lock().expect("runs").get(id).cloned()
        }

        fn cancel(&self, id: &str) -> Result<(), String> {
            let mut runs = self.runs.lock().expect("runs");
            let run = runs.get_mut(id).ok_or_else(|| "unknown run".to_string())?;
            run.state = RunState::Cancelled;
            Ok(())
        }
    }

    fn spec() -> RunSpec {
        RunSpec {
            session: "ses-1".to_string(),
            objective: "Review the working copy".to_string(),
            tier: RunTier::Subagent,
            limits: Limits::default(),
            target: None,
            capability: None,
            trace: "task-1".to_string(),
        }
    }

    /// An uninstalled seam refuses with a stable message.
    #[test]
    fn an_uninstalled_seam_refuses() {
        let runs = Runs::empty();
        assert!(!runs.is_installed());
        assert_eq!(runs.start(spec()).unwrap_err(), NO_SERVICE);
        assert_eq!(runs.cancel("run-1").unwrap_err(), NO_SERVICE);
        assert!(runs.observe("run-1").is_none());
    }

    /// A service receives the exact narrow spec and its run can be observed
    /// and cancelled.
    #[test]
    fn a_service_receives_the_spec_and_the_run_observes() {
        let fake = Arc::new(Fake::default());
        let runs = Runs::with(fake.clone());
        let requested = spec();
        let handle = runs.start(requested.clone()).expect("start");
        assert_eq!(handle.id, "run-1");
        assert_eq!(fake.specs.lock().expect("specs")[0], requested);

        let snapshot = runs.observe(&handle.id).expect("observe");
        assert_eq!(snapshot.state, RunState::Running);
        assert!(!snapshot.state.is_settled());

        runs.cancel(&handle.id).expect("cancel");
        let snapshot = runs.observe(&handle.id).expect("observe");
        assert_eq!(snapshot.state, RunState::Cancelled);
        assert!(snapshot.state.is_settled());
    }

    /// Incomplete specs are refused before any service sees them.
    #[test]
    fn invalid_specs_are_refused() {
        let mut empty_session = spec();
        empty_session.session = "  ".to_string();
        assert!(empty_session.validate().is_err());

        let mut empty_objective = spec();
        empty_objective.objective = String::new();
        assert!(empty_objective.validate().is_err());

        let mut empty_trace = spec();
        empty_trace.trace = String::new();
        assert!(empty_trace.validate().is_err());

        let mut stray_target = spec();
        stray_target.target = Some("node-b".to_string());
        assert!(stray_target.validate().is_err());

        let fake = Arc::new(Fake::default());
        let runs = Runs::with(fake.clone());
        assert!(runs.start(stray_target).is_err());
        assert_eq!(*fake.started.lock().expect("started"), 0);
    }

    /// A delegation needs a target and may carry a capability allowlist.
    #[test]
    fn a_delegation_needs_a_target() {
        let mut delegation = spec();
        delegation.tier = RunTier::Delegation;
        assert!(delegation.validate().is_err());
        delegation.target = Some("node-b".to_string());
        delegation.capability = Some("builtin/read_file".to_string());
        assert!(delegation.validate().is_ok());
    }

    /// The wire shape carries no model, preset, workspace or permission field.
    #[test]
    fn the_spec_is_narrow_only() {
        let value = serde_json::to_value(spec()).expect("json");
        let object = value.as_object().expect("an object");
        for forbidden in ["model", "preset", "workspace", "permission", "policy"] {
            assert!(
                !object.contains_key(forbidden),
                "the spec must not carry {forbidden}"
            );
        }
        assert_eq!(object["tier"], "subagent");
        assert_eq!(object["trace"], "task-1");
        assert!(object["limits"]["steps"].is_number());
    }

    /// The process seam installs, serves and clears.
    #[test]
    fn the_process_seam_installs_and_clears() {
        clear_runs_service();
        assert!(!runs().is_installed());
        install_runs_service(Arc::new(Fake::default()));
        assert!(runs().is_installed());
        let handle = runs().start(spec()).expect("start");
        assert!(runs().observe(&handle.id).is_some());
        clear_runs_service();
        assert!(!runs().is_installed());
        assert_eq!(runs().start(spec()).unwrap_err(), NO_SERVICE);
    }
}
