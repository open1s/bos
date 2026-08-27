//! Process manager: a `ResourceHandler` at `proc://` that multiplexes the
//! node's process table. `Spawn` here starts a child and registers it as
//! `proc://<pid>`, so any node that registers a manager gets a uniform,
//! cross-node `proc://` namespace.
//!
//! ```text
//! proc://            ← this manager (spawn, list, status)
//! proc://<pid>       ← individual process (kill, wait, status, events)
//! ```

use std::collections::HashMap;

use async_trait::async_trait;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;

use super::ProcResource;
use crate::action::{ResourceAction, ResourceOutput};
use crate::error::{ResourceError, Result};
use crate::handler::ResourceHandler;
use crate::manager::ResourceManager;
use crate::meta::{ResourceMeta, ResourceStateLabel, ResourceType};

/// One tracked process.
struct ProcEntry {
    /// URI under which the child is registered.
    uri: String,
    /// Command + arguments it was spawned with (for diagnostics/list).
    cmdline: String,
}

/// The node-local process manager.
///
/// `Spawn` is the only mutation; children become standalone `ProcResource`s.
/// The manager never reaps them itself: `Wait` on a child either returns
/// its exit code or times out on the server.
pub struct ProcManager {
    meta: ResourceMeta,
    /// Resource manager to register each spawned child into.
    mgr: Arc<ResourceManager>,
    procs: RwLock<HashMap<u32, ProcEntry>>,
}

impl ProcManager {
    /// Create the manager; registers itself nowhere, caller registers it at
    /// `proc://` on the same `mgr`.
    pub fn new(mgr: Arc<ResourceManager>) -> Self {
        Self {
            meta: ResourceMeta {
                uri: "proc://".into(),
                kind: ResourceType::Compute,
                state: ResourceStateLabel::Open,
                owner: String::new(),
                metadata: None,
            },
            mgr,
            procs: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl ResourceHandler for ProcManager {
    fn meta(&self) -> &ResourceMeta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut ResourceMeta {
        &mut self.meta
    }

    async fn handle(&mut self, action: ResourceAction) -> Result<ResourceOutput> {
        match action {
            ResourceAction::Spawn { args, env } => {
                if args.is_empty() {
                    return Err(ResourceError::Other(
                        "spawn requires at least a program".to_string(),
                    ));
                }
                let mut cmd = Command::new(&args[0]);
                cmd.args(&args[1..]);
                for (k, v) in &env {
                    cmd.env(k, v);
                }
                // Inherit stdio by default — the node's stdout/stderr get the
                // child's output (an rex agent watches its own stream).
                let child = cmd.spawn().map_err(ResourceError::Io)?;
                let pid = child
                    .id()
                    .ok_or_else(|| ResourceError::Other("no pid".to_string()))?;
                let uri = format!("proc://{pid}");
                let cmdline = args.join(" ");
                let resource = ProcResource::from_child(pid.to_string(), pid, child);
                self.mgr
                    .register(Box::new(resource), self.meta.owner.clone())
                    .await?;
                self.procs
                    .write()
                    .await
                    .insert(pid, ProcEntry { uri: uri.clone(), cmdline });
                Ok(ResourceOutput::Spawned { pid })
            }
            ResourceAction::List { .. } => {
                let procs = self.procs.read().await;
                let entries: Vec<String> = procs
                    .values()
                    .map(|p| format!("{} ({})", p.uri, p.cmdline))
                    .collect();
                Ok(ResourceOutput::Listed { entries })
            }
            ResourceAction::Status => {
                let running = self.procs.read().await.len();
                Ok(if running == 0 {
                    ResourceOutput::Status {
                        state: ResourceStateLabel::Closed,
                    }
                } else {
                    ResourceOutput::Status {
                        state: ResourceStateLabel::Open,
                    }
                })
            }
            other => Err(ResourceError::Unsupported(format!(
                "proc manager does not support {:?} (use proc://<pid> instead)",
                other.name()
            ))),
        }
    }
}
