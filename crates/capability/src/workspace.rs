//! An instantiated workspace: a resolved plan plus the providers it dispatches to.
//!
//! The runtime is the same shape at both levels. A local binding dispatches to
//! the provider that implements the capability here; a remote binding
//! dispatches to the provider registered for that peer, which proxies the call
//! across the federation. The workspace never decides which one it is holding:
//! the binding does, and it was decided by the resolver under policy.
//!
//! A workspace is one instance with one id. Using a peer does not create a
//! second workspace, and it does not merge the peer's authority into this one.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use crate::composition::{Binding, Budgets, ResolvedComposition};
use crate::descriptor::CapabilityDescriptor;
use crate::error::CapabilityError;
use crate::id::{CapabilityId, NodeId, ProviderId, WorkspaceId};
use crate::provider::{CapabilityCall, CapabilityOutput, CapabilityProvider};

/// The provider id a remote binding dispatches through.
///
/// A remote capability is a proxy: the peer's provider adapter registers under
/// this id and performs the authenticated federation call. The capability's own
/// id is not rewritten, because it still names the peer's capability.
pub fn remote_provider_id(node: &NodeId) -> ProviderId {
    ProviderId::new(format!("remote:{node}"))
}

/// Why a workspace could not be instantiated.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WorkspaceError {
    /// A binding named a provider that is not registered.
    #[error("no provider is registered for {provider}")]
    MissingProvider {
        /// The provider the plan needs.
        provider: ProviderId,
    },
    /// The plan binds one capability more than once.
    #[error("the plan binds {capability} more than once")]
    Duplicate {
        /// The capability bound twice.
        capability: CapabilityId,
    },
    /// The explicit activation order does not cover each binding exactly once.
    #[error("the activation order does not match the plan: {message}")]
    InvalidOrder {
        /// What is wrong.
        message: String,
    },
    /// The lifecycle state does not permit the requested operation.
    #[error("cannot {action} while the workspace is {state:?}")]
    InvalidState {
        /// The current state.
        state: WorkspaceState,
        /// The refused operation.
        action: &'static str,
    },
    /// One provider failed during activation; earlier ones were rolled back.
    #[error("provider {capability} failed to activate: {message}")]
    Activation {
        /// The provider capability that failed.
        capability: CapabilityId,
        /// The failure.
        message: String,
        /// Rollback failures, in reverse activation order.
        rollback_failures: Vec<String>,
    },
    /// In-flight calls did not drain within the deadline.
    #[error("{in_flight} invocation(s) are still running")]
    DrainTimeout {
        /// How many calls remain.
        in_flight: usize,
    },
    /// One or more providers failed to deactivate.
    #[error("teardown failures: {failures:?}")]
    Teardown {
        /// Every failure, in teardown order.
        failures: Vec<String>,
    },
    /// A resource is already owned by this workspace.
    #[error("resource {resource} is already owned")]
    DuplicateResource {
        /// The resource id.
        resource: String,
    },
}

/// The explicit lifecycle state of a managed workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceState {
    /// A runtime exists but no provider has been activated.
    Created,
    /// The plan is bound to providers and ready to activate.
    Prepared,
    /// Providers are being activated.
    Activating,
    /// Calls are admitted.
    Active,
    /// Admission is closed and calls are finishing.
    Draining,
    /// Admission is closed; the activation is retained.
    Suspended,
    /// Every provider has been deactivated; terminal.
    Stopped,
    /// Activation or teardown failed; a retry may be possible.
    Failed,
}

struct Dispatch {
    descriptor: CapabilityDescriptor,
    provider: Arc<dyn CapabilityProvider>,
    binding: Binding,
}

/// One instantiated workspace.
pub struct WorkspaceRuntime {
    workspace: WorkspaceId,
    capabilities: BTreeMap<CapabilityId, Dispatch>,
    budgets: Budgets,
    /// Dependency-first activation order.
    activation_order: Vec<CapabilityId>,
    /// Concurrency admission for this one workspace.
    slots: tokio::sync::Semaphore,
}

impl std::fmt::Debug for WorkspaceRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceRuntime")
            .field("workspace", &self.workspace)
            .field(
                "capabilities",
                &self.capabilities.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl WorkspaceRuntime {
    /// Bind a resolved plan to the providers that will execute it.
    ///
    /// A local binding needs its provider id; a remote binding needs the
    /// provider registered for that peer. Nothing here reaches a peer itself,
    /// so a missing remote provider fails at instantiation rather than on the
    /// first call.
    pub fn instantiate(
        plan: ResolvedComposition,
        providers: &[Arc<dyn CapabilityProvider>],
    ) -> Result<Self, WorkspaceError> {
        let mut order: Vec<CapabilityId> = plan
            .capabilities
            .iter()
            .map(|capability| capability.descriptor.id.clone())
            .collect();
        order.sort();
        Self::instantiate_ordered(plan, providers, &order)
    }

    /// Bind a plan with an explicit dependency-first activation order.
    ///
    /// The order must name every binding exactly once; a lock's activation
    /// order is the intended source.
    pub fn instantiate_ordered(
        plan: ResolvedComposition,
        providers: &[Arc<dyn CapabilityProvider>],
        activation_order: &[CapabilityId],
    ) -> Result<Self, WorkspaceError> {
        let mut capabilities = BTreeMap::new();
        for resolved in plan.capabilities {
            let id = resolved.descriptor.id.clone();
            let provider_id = match &resolved.binding {
                Binding::Local { provider } => provider.clone(),
                Binding::Remote { node, .. } => remote_provider_id(node),
            };
            let provider = providers
                .iter()
                .find(|provider| provider.id() == &provider_id)
                .cloned()
                .ok_or(WorkspaceError::MissingProvider {
                    provider: provider_id,
                })?;
            let dispatch = Dispatch {
                descriptor: resolved.descriptor,
                provider,
                binding: resolved.binding,
            };
            if capabilities.insert(id.clone(), dispatch).is_some() {
                return Err(WorkspaceError::Duplicate { capability: id });
            }
        }
        let ids: BTreeSet<CapabilityId> = capabilities.keys().cloned().collect();
        let ordered: BTreeSet<CapabilityId> = activation_order.iter().cloned().collect();
        if ids != ordered || ordered.len() != activation_order.len() {
            return Err(WorkspaceError::InvalidOrder {
                message: format!(
                    "expected each of {} bound capabilities exactly once",
                    ids.len()
                ),
            });
        }
        let budgets = plan.budgets.clone();
        Ok(Self {
            workspace: plan.workspace,
            capabilities,
            activation_order: activation_order.to_vec(),
            slots: tokio::sync::Semaphore::new(usize::from(budgets.max_concurrent_invocations)),
            budgets,
        })
    }

    /// The dependency-first activation order.
    pub fn activation_order(&self) -> &[CapabilityId] {
        &self.activation_order
    }

    /// The workspace this runtime instantiates.
    pub fn workspace(&self) -> &WorkspaceId {
        &self.workspace
    }

    /// The effective execution budgets this runtime enforces.
    pub fn budgets(&self) -> &Budgets {
        &self.budgets
    }

    /// The descriptors this workspace exposes, in id order.
    pub fn describe(&self) -> Vec<&CapabilityDescriptor> {
        self.capabilities
            .values()
            .map(|dispatch| &dispatch.descriptor)
            .collect()
    }

    /// Where one capability executes.
    pub fn binding(&self, id: &CapabilityId) -> Option<&Binding> {
        self.capabilities.get(id).map(|dispatch| &dispatch.binding)
    }

    /// Whether one capability is part of this workspace.
    pub fn contains(&self, id: &CapabilityId) -> bool {
        self.capabilities.contains_key(id)
    }

    /// Invoke one capability.
    ///
    /// An id outside the workspace is a NotFound, not a denial: the workspace
    /// exposes a subset, and a caller cannot ask it for something it was not
    /// built with.
    pub async fn invoke(
        &self,
        id: &CapabilityId,
        input: serde_json::Value,
        context: &crate::context::InvocationContext,
    ) -> Result<CapabilityOutput, CapabilityError> {
        self.invoke_operation(id, None, input, context).await
    }

    /// Invoke one operation of a multi-operation capability.
    pub async fn invoke_operation(
        &self,
        id: &CapabilityId,
        operation: Option<&str>,
        input: serde_json::Value,
        context: &crate::context::InvocationContext,
    ) -> Result<CapabilityOutput, CapabilityError> {
        // The workspace is the authorization boundary: a context that belongs
        // to another workspace must not reach this runtime's providers, local
        // or remote. Checked here once, not in each provider.
        if context.workspace != self.workspace {
            return Err(CapabilityError::Denied {
                reason: format!(
                    "the invocation belongs to workspace {}, not {}",
                    context.workspace, self.workspace
                ),
                policy: None,
            });
        }
        let Some(dispatch) = self.capabilities.get(id) else {
            return Err(CapabilityError::NotFound {
                capability: id.clone(),
            });
        };
        if context.cancelled() {
            return Err(CapabilityError::Cancelled);
        }
        let started = Instant::now();
        // The default timeout is a ceiling, not an extension: if the caller
        // supplied an earlier deadline, that one wins; if it supplied a later
        // one, the composition's ceiling still applies.
        let default_deadline = self
            .budgets
            .default_timeout
            .map(|timeout| started + timeout);
        let deadline = match (context.deadline, default_deadline) {
            (Some(caller), Some(configured)) => Some(caller.min(configured)),
            (Some(caller), None) => Some(caller),
            (None, Some(configured)) => Some(configured),
            (None, None) => None,
        };
        if deadline.is_some_and(|deadline| deadline <= started) {
            return Err(CapabilityError::Timeout {
                elapsed_ms: 0,
                partial: None,
            });
        }
        let mut effective = context.clone();
        effective.deadline = deadline;

        // Admission is per workspace and held for the whole call. A permit is
        // released by Drop on success, error, panic, cancellation, and abort.
        let permit = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match tokio::time::timeout(remaining, self.slots.acquire()).await {
                    Ok(Ok(permit)) => permit,
                    Ok(Err(_)) => {
                        return Err(CapabilityError::Unavailable {
                            reason: "the workspace dispatcher is closed".to_string(),
                        })
                    }
                    Err(_) => {
                        return Err(CapabilityError::Timeout {
                            elapsed_ms: started.elapsed().as_millis() as u64,
                            partial: None,
                        })
                    }
                }
            }
            None => match self.slots.acquire().await {
                Ok(permit) => permit,
                Err(_) => {
                    return Err(CapabilityError::Unavailable {
                        reason: "the workspace dispatcher is closed".to_string(),
                    })
                }
            },
        };
        if effective.cancelled() {
            return Err(CapabilityError::Cancelled);
        }

        let call = CapabilityCall {
            capability: id,
            operation,
            input,
            context: &effective,
        };
        let result = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match tokio::time::timeout(remaining, dispatch.provider.invoke(call)).await {
                    Ok(result) => result,
                    Err(_) => Err(CapabilityError::Timeout {
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        partial: None,
                    }),
                }
            }
            None => dispatch.provider.invoke(call).await,
        };
        drop(permit);
        result
    }
}
/// A managed workspace: a runtime plus its explicit lifecycle.
///
/// Admission, activation, drain, teardown, and resource ownership all happen
/// here. A workspace is never partially active: if a provider fails to
/// activate, the already-activated providers are deactivated in reverse order
/// and the lifecycle ends in Failed.
pub struct WorkspaceLifecycle {
    runtime: WorkspaceRuntime,
    state: Mutex<WorkspaceState>,
    journal: Mutex<Vec<CapabilityId>>,
    in_flight: AtomicUsize,
    idle: Notify,
    resources: Mutex<BTreeSet<String>>,
}

impl std::fmt::Debug for WorkspaceLifecycle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceLifecycle")
            .field("runtime", &self.runtime)
            .field("state", &self.state.lock().map(|state| *state))
            .field("in_flight", &self.in_flight.load(Ordering::SeqCst))
            .finish()
    }
}

impl WorkspaceLifecycle {
    /// Bind a plan and prepare its runtime without activating providers.
    pub fn new(
        plan: ResolvedComposition,
        providers: &[Arc<dyn CapabilityProvider>],
    ) -> Result<Self, WorkspaceError> {
        Self::prepare(WorkspaceRuntime::instantiate(plan, providers)?)
    }

    /// Bind a plan with an explicit activation order.
    pub fn with_order(
        plan: ResolvedComposition,
        providers: &[Arc<dyn CapabilityProvider>],
        activation_order: &[CapabilityId],
    ) -> Result<Self, WorkspaceError> {
        Self::prepare(WorkspaceRuntime::instantiate_ordered(
            plan,
            providers,
            activation_order,
        )?)
    }

    fn prepare(runtime: WorkspaceRuntime) -> Result<Self, WorkspaceError> {
        Ok(Self {
            runtime,
            state: Mutex::new(WorkspaceState::Prepared),
            journal: Mutex::new(Vec::new()),
            in_flight: AtomicUsize::new(0),
            idle: Notify::new(),
            resources: Mutex::new(BTreeSet::new()),
        })
    }

    /// The current lifecycle state.
    pub fn state(&self) -> WorkspaceState {
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The underlying dispatch table.
    pub fn runtime(&self) -> &WorkspaceRuntime {
        &self.runtime
    }

    /// Resources currently owned by this workspace, sorted.
    pub fn resources(&self) -> Vec<String> {
        self.resources
            .lock()
            .map(|resources| resources.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Register a resource this workspace owns. Released by shutdown.
    pub fn own_resource(&self, resource: impl Into<String>) -> Result<(), WorkspaceError> {
        let state = self.state();
        if state != WorkspaceState::Active && state != WorkspaceState::Activating {
            return Err(WorkspaceError::InvalidState {
                state,
                action: "own a resource",
            });
        }
        let resource = resource.into();
        let mut resources = self
            .resources
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !resources.insert(resource.clone()) {
            return Err(WorkspaceError::DuplicateResource { resource });
        }
        Ok(())
    }

    /// Activate every provider in dependency order.
    ///
    /// Idempotent while Active. A failure rolls back the already activated
    /// providers in reverse order and leaves the lifecycle in Failed.
    pub async fn activate(&mut self) -> Result<(), WorkspaceError> {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match *state {
                WorkspaceState::Active => return Ok(()),
                WorkspaceState::Prepared | WorkspaceState::Created => {
                    *state = WorkspaceState::Activating;
                }
                other => {
                    return Err(WorkspaceError::InvalidState {
                        state: other,
                        action: "activate",
                    })
                }
            }
        }
        let order = self.runtime.activation_order.clone();
        for id in order {
            let Some(provider) = self
                .runtime
                .capabilities
                .get(&id)
                .map(|dispatch| Arc::clone(&dispatch.provider))
            else {
                continue;
            };
            match provider.activate().await {
                Ok(()) => self
                    .journal
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(id),
                Err(error) => {
                    let activated: Vec<CapabilityId> = self
                        .journal
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .drain(..)
                        .collect();
                    let mut rollback_failures = Vec::new();
                    for done in activated.into_iter().rev() {
                        if let Some(provider) = self
                            .runtime
                            .capabilities
                            .get(&done)
                            .map(|dispatch| Arc::clone(&dispatch.provider))
                        {
                            if let Err(rollback) = provider.deactivate().await {
                                rollback_failures.push(format!("{done}: {rollback}"));
                            }
                        }
                    }
                    *self
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = WorkspaceState::Failed;
                    return Err(WorkspaceError::Activation {
                        capability: id,
                        message: error.to_string(),
                        rollback_failures,
                    });
                }
            }
        }
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = WorkspaceState::Active;
        Ok(())
    }

    /// Invoke a capability while holding an in-flight count.
    pub async fn invoke(
        &self,
        id: &CapabilityId,
        input: serde_json::Value,
        context: &crate::context::InvocationContext,
    ) -> Result<CapabilityOutput, CapabilityError> {
        {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *state != WorkspaceState::Active {
                return Err(CapabilityError::Denied {
                    reason: format!("the workspace is {state:?}"),
                    policy: None,
                });
            }
            // The count is taken under the state lock, so shutdown either sees
            // this call before it closes admission or refuses it.
            self.in_flight.fetch_add(1, Ordering::SeqCst);
        }
        let _guard = InFlight { lifecycle: self };
        self.runtime
            .invoke_operation(id, None, input, context)
            .await
    }

    /// Close admission, wait for in-flight calls, then deactivate providers.
    ///
    /// Idempotent once Stopped. A drain timeout leaves the lifecycle Draining
    /// so the caller can retry rather than tearing down under running calls.
    pub async fn shutdown(&self, timeout: Duration) -> Result<(), WorkspaceError> {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match *state {
                WorkspaceState::Stopped => return Ok(()),
                WorkspaceState::Active
                | WorkspaceState::Suspended
                | WorkspaceState::Failed
                | WorkspaceState::Draining => {
                    *state = WorkspaceState::Draining;
                }
                other => {
                    return Err(WorkspaceError::InvalidState {
                        state: other,
                        action: "shut down",
                    })
                }
            }
        }
        if !self.wait_idle(timeout).await {
            return Err(WorkspaceError::DrainTimeout {
                in_flight: self.in_flight.load(Ordering::SeqCst),
            });
        }
        let activated: Vec<CapabilityId> = self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
            .collect();
        let mut failures = Vec::new();
        for id in activated.into_iter().rev() {
            if let Some(provider) = self
                .runtime
                .capabilities
                .get(&id)
                .map(|dispatch| Arc::clone(&dispatch.provider))
            {
                if let Err(error) = provider.deactivate().await {
                    failures.push(format!("{id}: {error}"));
                }
            }
        }
        // Resource ownership is released exactly once, after providers stop.
        self.resources
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = WorkspaceState::Stopped;
        if failures.is_empty() {
            Ok(())
        } else {
            Err(WorkspaceError::Teardown { failures })
        }
    }

    /// Close admission and wait for in-flight calls, keeping the activation.
    ///
    /// On a drain timeout the lifecycle returns to Active.
    pub async fn suspend(&self, timeout: Duration) -> Result<(), WorkspaceError> {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *state != WorkspaceState::Active {
                return Err(WorkspaceError::InvalidState {
                    state: *state,
                    action: "suspend",
                });
            }
            *state = WorkspaceState::Suspended;
        }
        if self.wait_idle(timeout).await {
            Ok(())
        } else {
            *self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = WorkspaceState::Active;
            Err(WorkspaceError::DrainTimeout {
                in_flight: self.in_flight.load(Ordering::SeqCst),
            })
        }
    }

    /// Reopen admission after a suspend.
    pub fn resume(&self) -> Result<(), WorkspaceError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *state != WorkspaceState::Suspended {
            return Err(WorkspaceError::InvalidState {
                state: *state,
                action: "resume",
            });
        }
        *state = WorkspaceState::Active;
        Ok(())
    }

    async fn wait_idle(&self, timeout: Duration) -> bool {
        let wait = async {
            loop {
                let notified = self.idle.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.in_flight.load(Ordering::SeqCst) == 0 {
                    break;
                }
                notified.await;
            }
        };
        tokio::time::timeout(timeout, wait).await.is_ok()
    }
}

struct InFlight<'a> {
    lifecycle: &'a WorkspaceLifecycle,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.lifecycle.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.lifecycle.idle.notify_waiters();
    }
}
