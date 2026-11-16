//! Compute-class resources: child processes, addressable as `proc://`.
//!
//! Process handling is portable across macOS, Linux, and Windows via
//! `tokio::process`. Termination *intent* is captured by [`Terminate`]
//! (graceful vs force vs raw Unix signal); `tokio::process::Child::start_kill`
//! maps it to `SIGKILL` / `TerminateProcess` as appropriate, so the semantics
//! stay consistent without leaking POSIX specifics.
//!
//! The child is owned by a dedicated *reaper task*: the single owner of the
//! `Child` handle, which serves kill requests from a channel and broadcasts
//! the exit status on a `watch` channel. This lets `Wait` (possibly several
//! concurrent callers), event subscribers, and peeks at status all observe the
//! same underlying process without touching the handle itself.

pub mod manager;
pub mod supervisor;

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch, Mutex};

use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::meta::{ResourceMeta, ResourceStateLabel, ResourceType};
use crate::platform::Terminate;

/// Command delivered to the reaper task (only kill waits remain outside the
/// reaper; `Wait` subscribes to the broadcast channel instead).
enum ProcCmd {
    Kill,
}

/// Shared handle to a running (or exited) child, owned by its reaper task.
struct ProcHandle {
    cmd: mpsc::UnboundedSender<ProcCmd>,
    exit: watch::Receiver<Option<i32>>,
}

/// Spawn `child` under a reaper task; returns a handle for control/observation.
fn spawn_reaper(mut child: Child) -> ProcHandle {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<ProcCmd>();
    let (exit_tx, exit_rx) = watch::channel::<Option<i32>>(None);
    tokio::spawn(async move {
        let code = loop {
            tokio::select! {
                status = child.wait() => break status.ok().and_then(|s| s.code()).unwrap_or(-1),
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(ProcCmd::Kill) => {
                            let _ = child.start_kill();
                        }
                        None => {
                            // All handles dropped: kill so no zombie lingers.
                            let _ = child.start_kill();
                        }
                    }
                }
            }
        };
        let _ = exit_tx.send(Some(code));
    });
    ProcHandle {
        cmd: cmd_tx,
        exit: exit_rx,
    }
}

/// A child-process resource (`proc://<name>`).
///
/// `Spawn` on a bare resource starts the program named by `args[0]`;
/// `Kill`/`SendSignal` terminate it; `Wait` awaits exit and returns the code.
pub struct ProcResource {
    meta: ResourceMeta,
    inner: Mutex<Option<ProcHandle>>,
}

impl ProcResource {
    /// Create an empty process resource (spawn on `Spawn`).
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let uri = format!("proc://{name}");
        let meta = ResourceMeta {
            uri,
            kind: ResourceType::Compute,
            state: ResourceStateLabel::Closed,
            owner: String::new(),
            metadata: None,
        };
        Self {
            meta,
            inner: Mutex::new(None),
        }
    }

    /// Wrap an already-spawned `child` as a resource named `name`.
    pub fn from_child(name: impl Into<String>, _pid: u32, child: Child) -> Self {
        let handle = spawn_reaper(child);
        let name = name.into();
        let meta = ResourceMeta {
            uri: format!("proc://{name}"),
            kind: ResourceType::Compute,
            state: ResourceStateLabel::Open,
            owner: String::new(),
            metadata: None,
        };
        Self {
            meta,
            inner: Mutex::new(Some(handle)),
        }
    }
}

#[async_trait]
impl ResourceHandler for ProcResource {
    fn meta(&self) -> &ResourceMeta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut ResourceMeta {
        &mut self.meta
    }

    async fn handle(&mut self, action: ResourceAction) -> Result<ResourceOutput> {
        let mut inner = self.inner.lock().await;
        match action {
            ResourceAction::Open => {
                self.meta.state = ResourceStateLabel::Open;
                Ok(ResourceOutput::Opened)
            }
            ResourceAction::Close => {
                if let Some(h) = inner.take() {
                    // Unbounded send never blocks; if the reaper is already
                    // gone we have nothing to wait for.
                    let _ = h.cmd.send(ProcCmd::Kill);
                }
                self.meta.state = ResourceStateLabel::Closed;
                Ok(ResourceOutput::Closed)
            }
            ResourceAction::Status => Ok(ResourceOutput::Status {
                state: self.meta.state,
            }),
            ResourceAction::Spawn { args, env } => {
                if args.is_empty() {
                    return Err(ResourceError::Other(
                        "spawn requires at least a program".to_string(),
                    ));
                }
                let mut cmd = Command::new(&args[0]);
                if args.len() > 1 {
                    cmd.args(&args[1..]);
                }
                for (k, v) in &env {
                    cmd.env(k, v);
                }
                let child = cmd.spawn().map_err(ResourceError::Io)?;
                let pid = child
                    .id()
                    .ok_or_else(|| ResourceError::Other("no pid".to_string()))?;
                *inner = Some(spawn_reaper(child));
                self.meta.state = ResourceStateLabel::Open;
                Ok(ResourceOutput::Spawned { pid })
            }
            ResourceAction::Kill { signal } => {
                let h = inner.as_ref().ok_or(ResourceError::Closed)?;
                // `start_kill` is SIGKILL on Unix and TerminateProcess on
                // Windows. The signal number is treated as a *force* hint; a
                // "graceful" intent would need a platform-specific delivery
                // mechanism (e.g. `kill` on Unix), which `start_kill` does not
                // expose. See [`Terminate`].
                let _ = signal;
                h.cmd
                    .send(ProcCmd::Kill)
                    .map_err(|_| ResourceError::Closed)?;
                Ok(ResourceOutput::Killed)
            }
            ResourceAction::SendSignal { signal } => {
                let h = inner.as_ref().ok_or(ResourceError::Closed)?;
                // System-class signal delivery. On Unix, SIGKILL-equivalent via
                // `start_kill`; a non-force signal is only honorably delivered
                // via `kill(2)`, which `tokio::process` does not expose portably.
                // We normalize intent through [`Terminate`] and document the
                // degradation: non-force signals collapse to termination.
                let _term = Terminate::Unix(signal);
                h.cmd
                    .send(ProcCmd::Kill)
                    .map_err(|_| ResourceError::Closed)?;
                Ok(ResourceOutput::SignalSent)
            }
            ResourceAction::Wait => {
                // Any number of waiters, at any time: subscribe to the exit
                // broadcast and block until it carries a code.
                let mut rx = inner.as_ref().ok_or(ResourceError::Closed)?.exit.clone();
                drop(inner); // release the handler lock while the process runs
                let code = loop {
                    let cur = *rx.borrow();
                    if let Some(code) = cur {
                        break code;
                    }
                    rx.changed().await.map_err(|_| ResourceError::Closed)?;
                };
                self.meta.state = ResourceStateLabel::Closed;
                Ok(ResourceOutput::Exited { code })
            }
            other => Err(ResourceError::Unsupported(format!(
                "proc resource does not support {:?}",
                other.name()
            ))),
        }
    }

    fn events(&mut self) -> Option<Pin<Box<dyn Stream<Item = ResourceEvent> + Send + 'static>>> {
        let exit = self.inner.try_lock().ok()?.as_ref()?.exit.clone();
        let stream = async_stream::stream! {
            let mut exit = exit;
            loop {
                // `watch::Receiver::borrow()` holds an internal RwLock read
                // guard that is !Send; copy the value out before any await.
                let cur = *exit.borrow();
                match cur {
                    Some(code) => {
                        yield ResourceEvent::Exited(code);
                        break;
                    }
                    None => {
                        if exit.changed().await.is_err() {
                            break;
                        }
                    }
                }
            }
        };
        Some(Box::pin(stream))
    }
}
