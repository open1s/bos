//! Process supervision: restart policies, restart budgets, and escalation,
//! Erlang-style but scoped deliberately small.
//!
//! A [`Supervisor`] is a `ResourceHandler` registered at `sup://<name>`.
//! It owns a group of child specs; on `Open` it spawns every child through
//! the node's `proc://` manager and starts one monitor task per child. When
//! a child exits, the monitor consults the [`RestartPolicy`]:
//!
//! | Policy | Exit 0 | Exit != 0 |
//! |--------|--------|-----------|
//! | `Always` | respawn | respawn |
//! | `OnFailure` | done | respawn |
//! | `Never` | done | done |
//!
//! A respawn counts against a budget (`max_restarts` within `window`).
//! Exceeding it *escalates*: the whole group is torn down (remaining children
//! killed, monitor tasks stopped) and the supervisor reports `Closed` plus a
//! `StateChanged` event.
//!
//! Health signal = exit status. There are no separate liveness probes: a
//! process that is running is healthy by definition.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use std::pin::Pin;
use tokio::sync::{watch, Mutex};

use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::manager::ResourceManager;
use crate::meta::{ResourceMeta, ResourceStateLabel, ResourceType};
use crate::transport::Dispatcher;

/// When a child exits, what should happen?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Respawn regardless of exit code.
    Always,
    /// Respawn only on non-zero exit.
    OnFailure,
    /// Never respawn; an exit is final.
    Never,
}

/// Supervision group configuration.
#[derive(Debug, Clone)]
pub struct SupPolicy {
    pub restart: RestartPolicy,
    /// Respawns allowed within `window` per child before escalation.
    pub max_restarts: u32,
    /// Rolling window for the restart budget.
    pub window: Duration,
}

impl Default for SupPolicy {
    fn default() -> Self {
        Self {
            restart: RestartPolicy::OnFailure,
            max_restarts: 3,
            window: Duration::from_secs(60),
        }
    }
}

/// One supervised program.
#[derive(Debug, Clone)]
pub struct ChildSpec {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Live state of one supervised child.
struct ChildState {
    /// Current pid, if running.
    pid: Option<u32>,
    /// Restart timestamps still inside the rolling window.
    restarts: Vec<Instant>,
    /// Set once the child has exited and the policy says "done" (or the
    /// group was torn down).
    finished: bool,
}

struct GroupState {
    children: Vec<ChildState>,
    /// True while the group is meant to be running.
    running: AtomicBool,
    /// Set by the monitor that escalated (budget exhausted).
    failed: bool,
    /// Broadcast when the group escalates to failed (for events()).
    failed_tx: watch::Sender<bool>,
}

/// Per-monitor context: everything a child-supervision task needs besides the
/// group itself. Bundles call-site values so `monitor` stays readable.
struct MonitorCtx {
    sup_uri: String,
    mgr: Arc<ResourceManager>,
    owner: String,
}

/// Spawn one child via the node's `proc://` manager; returns its pid.
async fn spawn_child(
    mgr: &Arc<ResourceManager>,
    owner: &str,
    idx: usize,
    spec: &ChildSpec,
) -> Result<u32> {
    let out = mgr
        .dispatch(
            owner,
            "proc://",
            ResourceAction::Spawn {
                args: spec.args.clone(),
                env: spec.env.clone(),
            },
        )
        .await?;
    match out {
        ResourceOutput::Spawned { pid } => Ok(pid),
        other => Err(ResourceError::Other(format!(
            "spawn child {idx}: unexpected {other:?}"
        ))),
    }
}

/// Kill (and unregister) a specific set of child URIs — only this group's,
/// never other processes on the node.
async fn kill_children(mgr: &Arc<ResourceManager>, owner: &str, uris: Vec<String>) {
    for uri in uris {
        let _ = mgr
            .dispatch(owner, &uri, ResourceAction::Kill { signal: 9 })
            .await;
        let _ = mgr.deregister(&uri).await;
    }
}

/// Monitor loop for one child. Runs until the group shuts down or the policy
/// declares the child finished.
async fn monitor(
    ctx: MonitorCtx,
    idx: usize,
    spec: ChildSpec,
    policy: SupPolicy,
    state: Arc<Mutex<GroupState>>,
) {
    let MonitorCtx {
        sup_uri,
        mgr,
        owner,
    } = ctx;
    loop {
        // Snapshot this child's URI and the group's run flag.
        let (my_uri, group_running) = {
            let g = state.lock().await;
            let uri = match g.children[idx].pid {
                Some(pid) => format!("proc://{pid}"),
                None => return, // never spawned (shouldn't happen)
            };
            (uri, g.running.load(Ordering::Relaxed))
        };
        if !group_running {
            return;
        }

        // `Wait` would hold the proc slot's lock for the whole process
        // lifetime, blocking Kill/Close from reaching the handler. Instead we
        // *subscribe* — subscription returns a stream immediately, so the slot
        // is free while we await the exit event.
        let mut events = match mgr.subscribe(&owner, &my_uri, vec![]).await {
            Ok(s) => s,
            Err(_) => {
                // Resource vanished (e.g. already deregistered): treat as a
                // failed exit and let the policy decide.
                let exit_code = -1;
                let mut g = state.lock().await;
                if policy.restart == RestartPolicy::Never || exit_code == 0 {
                    g.children[idx].finished = true;
                    return;
                }
                drop(g);
                continue;
            }
        };
        let mut exit_code = -1;
        while let Some(ev) = events.next().await {
            if let ResourceEvent::Exited(code) = ev {
                exit_code = code;
                break;
            }
        }

        let should_respawn = match policy.restart {
            RestartPolicy::Always => true,
            RestartPolicy::OnFailure => exit_code != 0,
            RestartPolicy::Never => false,
        };

        let mut g = state.lock().await;
        g.children[idx].pid = None;
        if !g.running.load(Ordering::Relaxed) || !should_respawn {
            g.children[idx].finished = true;
            return;
        }

        // Budget: drop restarts outside the rolling window, then check the cap.
        let now = Instant::now();
        g.children[idx]
            .restarts
            .retain(|t| now.duration_since(*t) < policy.window);
        if g.children[idx].restarts.len() >= policy.max_restarts as usize {
            // Escalate: kill the whole group. Only this group's children —
            // other supervisors on the same node are untouched.
            g.running.store(false, Ordering::Relaxed);
            g.failed = true;
            let _ = g.failed_tx.send(true);
            let uris: Vec<String> = g
                .children
                .iter()
                .filter_map(|c| c.pid.map(|pid| format!("proc://{pid}")))
                .collect();
            drop(g);
            kill_children(&mgr, &owner, uris).await;
            return;
        }
        g.children[idx].restarts.push(now);

        // Unregister the dead proc resource, then respawn.
        drop(g);
        let _ = mgr.deregister(&my_uri).await;
        match spawn_child(&mgr, &owner, idx, &spec).await {
            Ok(pid) => {
                let mut g = state.lock().await;
                g.children[idx].pid = Some(pid);
            }
            Err(e) => {
                tracing::warn!("{sup_uri}: respawn child {idx} failed: {e}");
                let mut g = state.lock().await;
                g.children[idx].finished = true;
                return;
            }
        }
    }
}

/// Supervisor: owns a group of children and restarts them per policy.
pub struct Supervisor {
    meta: ResourceMeta,
    mgr: Arc<ResourceManager>,
    policy: SupPolicy,
    specs: Vec<ChildSpec>,
    state: Arc<Mutex<GroupState>>,
}

impl Supervisor {
    /// Create a supervisor for `name` (`sup://<name>`).
    pub fn new(
        name: impl Into<String>,
        mgr: Arc<ResourceManager>,
        specs: Vec<ChildSpec>,
        policy: SupPolicy,
    ) -> Self {
        let name = name.into();
        Self {
            meta: ResourceMeta {
                uri: format!("sup://{name}"),
                kind: ResourceType::Compute,
                state: ResourceStateLabel::Closed,
                owner: String::new(),
                metadata: None,
            },
            mgr,
            policy,
            specs,
            state: Arc::new(Mutex::new(GroupState {
                children: vec![],
                running: AtomicBool::new(false),
                failed: false,
                failed_tx: watch::channel(false).0,
            })),
        }
    }

    /// The current group failed-tx (for tests / diagnostics).
    pub fn failed_rx(&self) -> watch::Receiver<bool> {
        // `failed_tx` lives inside the lock; grab the sender cheaply via a
        // try_lock and subscribe.
        self.state
            .try_lock()
            .map(|g| g.failed_tx.subscribe())
            .unwrap_or_else(|_| {
                let (tx, rx) = watch::channel(false);
                let _ = tx; // offline fallback: never fires
                rx
            })
    }
}

#[async_trait]
impl ResourceHandler for Supervisor {
    fn meta(&self) -> &ResourceMeta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut ResourceMeta {
        &mut self.meta
    }

    async fn handle(&mut self, action: ResourceAction) -> Result<ResourceOutput> {
        match action {
            ResourceAction::Open => {
                let (specs, policy, owner, already) = {
                    let mut g = self.state.lock().await;
                    let already = g.running.swap(true, Ordering::SeqCst);
                    if !already {
                        g.children = self
                            .specs
                            .iter()
                            .map(|_| ChildState {
                                pid: None,
                                restarts: Vec::new(),
                                finished: false,
                            })
                            .collect();
                    }
                    let owner = if self.meta.owner.is_empty() {
                        "supervisor".to_string()
                    } else {
                        self.meta.owner.clone()
                    };
                    (self.specs.clone(), self.policy.clone(), owner, already)
                };
                if already {
                    return Ok(ResourceOutput::Opened);
                }

                // Spawn all children.
                for (idx, spec) in specs.iter().enumerate() {
                    match spawn_child(&self.mgr, &owner, idx, spec).await {
                        Ok(pid) => {
                            let mut g = self.state.lock().await;
                            g.children[idx].pid = Some(pid);
                        }
                        Err(e) => {
                            self.state
                                .lock()
                                .await
                                .running
                                .store(false, Ordering::SeqCst);
                            return Err(e);
                        }
                    }
                }
                // Start one monitor per child.
                for (idx, spec) in specs.iter().enumerate() {
                    let ctx = MonitorCtx {
                        sup_uri: self.meta.uri.clone(),
                        mgr: self.mgr.clone(),
                        owner: owner.clone(),
                    };
                    let st = self.state.clone();
                    let spec = spec.clone();
                    let policy = policy.clone();
                    tokio::spawn(async move {
                        monitor(ctx, idx, spec, policy, st).await;
                    });
                }
                self.meta.state = ResourceStateLabel::Open;
                Ok(ResourceOutput::Opened)
            }
            ResourceAction::Close => {
                let (uris, owner) = {
                    let mut g = self.state.lock().await;
                    g.running.store(false, Ordering::SeqCst);
                    // Mark synchronously: monitor tasks will confirm via the
                    // exit event, but Close's contract is that after it
                    // returns the group is down.
                    let uris: Vec<String> = g
                        .children
                        .iter_mut()
                        .filter_map(|c| {
                            c.finished = true;
                            c.pid.take().map(|pid| format!("proc://{pid}"))
                        })
                        .collect();
                    let owner = if self.meta.owner.is_empty() {
                        "supervisor".to_string()
                    } else {
                        self.meta.owner.clone()
                    };
                    (uris, owner)
                };
                kill_children(&self.mgr, &owner, uris).await;
                self.meta.state = ResourceStateLabel::Closed;
                Ok(ResourceOutput::Closed)
            }
            ResourceAction::Status => {
                let g = self.state.lock().await;
                Ok(ResourceOutput::Status {
                    state: if g.failed {
                        ResourceStateLabel::Closed
                    } else if g.running.load(Ordering::Relaxed) {
                        ResourceStateLabel::Open
                    } else {
                        self.meta.state
                    },
                })
            }
            ResourceAction::List { .. } => {
                let g = self.state.lock().await;
                let entries: Vec<String> = g
                    .children
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let live = c
                            .pid
                            .map(|p| format!("proc://{p}"))
                            .unwrap_or_else(|| "-".into());
                        let status = if c.finished { "done" } else { "running" };
                        format!("[{i}] {live} {status} restarts={}", c.restarts.len())
                    })
                    .collect();
                Ok(ResourceOutput::Listed { entries })
            }
            other => Err(ResourceError::Unsupported(format!(
                "supervisor does not support {:?}",
                other.name()
            ))),
        }
    }

    fn events(&mut self) -> Option<Pin<Box<dyn Stream<Item = ResourceEvent> + Send + 'static>>> {
        let rx = self.failed_rx();
        let stream = async_stream::stream! {
            let mut rx = rx;
            loop {
                if *rx.borrow() {
                    yield ResourceEvent::StateChanged(ResourceStateLabel::Closed);
                    break;
                }
                if rx.changed().await.is_err() {
                    break;
                }
            }
        };
        Some(Box::pin(stream))
    }
}
