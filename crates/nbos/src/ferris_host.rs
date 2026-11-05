//! Full Ferris host binding: package store, activation and standard runtimes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use pyo3::prelude::*;

use crate::ferris::{call_python, PyForeignExtension};
use crate::utils::{json_to_py, py_to_json};
use ferris_bindings::{
    event_runtime, parse_capability, parse_policy_mode, parse_scope, parse_trust,
    AppliedContributions, BindingHostSpec, BindingRuntimeOptions, BindingRuntimeResolver,
    ContributionCallback, HostContributionSink, NoopLeaseHost, ShellTerminal,
};
use ferris_sdk::capability::PolicyEngine;
use ferris_sdk::events::{EventBus, EventFilter};
use ferris_sdk::manager::{Activation, ExtensionManager};
use ferris_sdk::types::ExtensionSource;
use ferris_sdk::{
    ContributionLease, ContributionRef, ContributionSink, ExtensionManifest, ExtensionRuntime,
    InvokeContext, Runtime,
};

/// One live event subscription and its reader task.
type SubscriptionSlot = Arc<StdMutex<Vec<(u32, tokio::task::JoinHandle<()>)>>>;

fn host_error(error: impl std::fmt::Display) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
}

/// Forward contribution lifecycle events to Python.
struct PyContributionCallback {
    on_apply: Option<Py<PyAny>>,
    on_revoke: Option<Py<PyAny>>,
}

#[async_trait]
impl ContributionCallback for PyContributionCallback {
    async fn applied(
        &self,
        extension: &str,
        contributions: &[ferris_sdk::Contribution],
    ) -> Result<(), String> {
        if let Some(callback) = &self.on_apply {
            call_python(
                callback,
                serde_json::json!({ "extension": extension, "contributions": contributions }),
            )
            .await
            .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    async fn revoked(
        &self,
        extension: &str,
        contributions: &[ferris_sdk::Contribution],
    ) -> Result<(), String> {
        if let Some(callback) = &self.on_revoke {
            call_python(
                callback,
                serde_json::json!({ "extension": extension, "contributions": contributions }),
            )
            .await
            .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

/// A Ferris package store and runtime host.
#[pyclass(name = "FerrisHost", skip_from_py_object)]
pub struct PyFerrisHost {
    root: PathBuf,
    manager: Arc<ExtensionManager>,
    policy: Arc<PolicyEngine>,
    events: Arc<EventBus>,
    book: Arc<AppliedContributions>,
    sink: Arc<HostContributionSink>,
    activations: Arc<StdMutex<Vec<Activation>>>,
    linked: Arc<StdMutex<Vec<PyLinkedActivation>>>,
    subscriptions: SubscriptionSlot,
    next_subscription: Arc<AtomicU32>,
}

/// One extension implemented in Python and hosted by this process.
struct PyLinkedActivation {
    extension: String,
    runtime: Arc<ExtensionRuntime<PyForeignExtension>>,
    lease: Box<dyn ContributionLease>,
}

impl Drop for PyFerrisHost {
    fn drop(&mut self) {
        if let Ok(subscriptions) = self.subscriptions.lock() {
            for (_, handle) in subscriptions.iter() {
                handle.abort();
            }
        }
    }
}

#[pymethods]
impl PyFerrisHost {
    #[new]
    #[pyo3(signature = (root, workspace = None, trust = None, policy_mode = None, on_contributions = None, on_revoke = None, process_sandbox = None))]
    fn new(
        root: String,
        workspace: Option<String>,
        trust: Option<String>,
        policy_mode: Option<String>,
        on_contributions: Option<Py<PyAny>>,
        on_revoke: Option<Py<PyAny>>,
        process_sandbox: Option<bool>,
    ) -> PyResult<Self> {
        let root = PathBuf::from(root);
        std::fs::create_dir_all(&root).map_err(host_error)?;
        let policy_root = root.join("policy");
        std::fs::create_dir_all(&policy_root).map_err(host_error)?;
        let policy = Arc::new(
            PolicyEngine::open(
                policy_root.join("grants.toml"),
                policy_root.join("audit.jsonl"),
                parse_policy_mode(policy_mode.as_deref()).map_err(host_error)?,
            )
            .map_err(host_error)?,
        );
        let events = EventBus::new();
        let trust = parse_trust(trust.as_deref()).map_err(host_error)?;
        let workspace = workspace.map(PathBuf::from);
        let resolver = Arc::new(BindingRuntimeResolver::new(BindingRuntimeOptions {
            workspace: workspace.clone(),
            policy: Arc::clone(&policy),
            events: Arc::clone(&events),
            trust,
            terminal: Arc::new(ShellTerminal),
            process_sandbox: process_sandbox.unwrap_or(true),
        }));
        let book = Arc::new(AppliedContributions::default());
        let callback = if on_contributions.is_some() || on_revoke.is_some() {
            Some(Arc::new(PyContributionCallback {
                on_apply: on_contributions,
                on_revoke,
            }) as Arc<dyn ContributionCallback>)
        } else {
            None
        };
        let sink = Arc::new(HostContributionSink::new(Arc::clone(&book), callback));
        let spec = Arc::new(BindingHostSpec::new(root.clone()));
        let sink_trait: Arc<dyn ContributionSink> = sink.clone();
        let manager = ExtensionManager::open(spec)
            .map_err(host_error)?
            .with_ports(resolver, sink_trait);
        Ok(Self {
            root,
            manager: Arc::new(manager),
            policy,
            events,
            book,
            sink,
            activations: Arc::new(StdMutex::new(Vec::new())),
            linked: Arc::new(StdMutex::new(Vec::new())),
            subscriptions: Arc::new(StdMutex::new(Vec::new())),
            next_subscription: Arc::new(AtomicU32::new(1)),
        })
    }

    #[getter]
    /// The canonical store root.
    fn root(&self) -> String {
        self.root.to_string_lossy().to_string()
    }

    /// Installed id/version pairs.
    fn installed(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let entries = self.manager.installed().map_err(host_error)?;
        let value = serde_json::json!(entries
            .iter()
            .map(|entry| serde_json::json!({ "id": entry.id, "version": entry.version }))
            .collect::<Vec<_>>());
        json_to_py(py, &value)
    }

    /// Install one package tree and return its identity.
    fn install(&self, py: Python<'_>, source: String) -> PyResult<Py<PyAny>> {
        let installed = self
            .manager
            .install(&PathBuf::from(source))
            .map_err(host_error)?;
        json_to_py(
            py,
            &serde_json::json!({ "id": installed.id, "version": installed.version }),
        )
    }

    /// Install one package tree with an explicit source label.
    #[pyo3(signature = (source, origin = None))]
    fn install_package(
        &self,
        py: Python<'_>,
        source: String,
        origin: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let origin = match origin.unwrap_or_else(|| "installed".to_string()).as_str() {
            "development" | "dev" => ExtensionSource::Development,
            "builtin" => ExtensionSource::Builtin,
            _ => ExtensionSource::Installed,
        };
        let package = self
            .manager
            .install_package(&PathBuf::from(source), origin)
            .map_err(host_error)?;
        json_to_py(
            py,
            &serde_json::json!({
                "id": package.id().as_str(),
                "version": package.version(),
            }),
        )
    }

    /// Remove one installed extension, optionally purging its data.
    #[pyo3(signature = (id, purge_data = false))]
    fn uninstall(&self, id: String, purge_data: bool) -> PyResult<()> {
        self.manager.uninstall(&id, purge_data).map_err(host_error)
    }

    /// Enable or disable one installed extension.
    fn set_enabled(&self, id: String, enabled: bool) -> PyResult<()> {
        self.manager.set_enabled(&id, enabled).map_err(host_error)
    }

    /// Every installed version of one extension, ascending.
    fn versions(&self, id: String) -> PyResult<Vec<String>> {
        self.manager.versions(&id).map_err(host_error)
    }

    /// The active version of one extension.
    fn active_version(&self, id: String) -> PyResult<Option<String>> {
        self.manager.active_version(&id).map_err(host_error)
    }

    /// Activate one installed version.
    fn activate(&self, id: String, version: String) -> PyResult<()> {
        self.manager.activate(&id, &version).map_err(host_error)
    }

    /// Activate the version before the current one.
    fn rollback(&self, id: String) -> PyResult<String> {
        self.manager.rollback(&id).map_err(host_error)
    }

    /// Clear a persisted failure for one extension.
    fn clear_failure(&self, id: String) -> PyResult<()> {
        self.manager.clear_failure(&id).map_err(host_error)
    }

    /// Move one extension's code to quarantine.
    fn quarantine(&self, id: String, reason: String) -> PyResult<String> {
        self.manager
            .quarantine(&id, &reason)
            .map(|path| path.to_string_lossy().to_string())
            .map_err(host_error)
    }

    /// The canonical package root for one installed version.
    fn package_root(&self, id: String, version: String) -> PyResult<String> {
        self.manager
            .package_root(&id, &version)
            .map(|path| path.to_string_lossy().to_string())
            .map_err(host_error)
    }

    /// Resolve installed packages into an activation order.
    fn plan(&self) -> PyResult<Vec<String>> {
        self.manager
            .plan()
            .map(|plan| plan.order)
            .map_err(host_error)
    }

    /// Start every planned package and return the applied contributions.
    fn apply<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let manager = Arc::clone(&self.manager);
        let book = Arc::clone(&self.book);
        let activations_store = Arc::clone(&self.activations);
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let plan = manager.plan().map_err(host_error)?;
            unload_activations(&activations_store).await?;
            let order = plan.order.clone();
            let mut agent_context = ();
            let mut registry_context = ();
            let mut lease_host = NoopLeaseHost;
            let mut activation_set = manager
                .apply(
                    &plan,
                    &mut agent_context,
                    &mut registry_context,
                    &mut lease_host,
                )
                .await
                .map_err(host_error)?;
            let mut started = Vec::new();
            for id in order {
                if let Some(activation) = activation_set.take(&id) {
                    started.push(activation);
                }
            }
            if let Ok(mut activations) = activations_store.lock() {
                activations.extend(started);
            }
            Python::attach(|py| json_to_py(py, &book.to_json()))
        })
    }

    /// Re-apply the current plan (unload, then start again).
    fn reload<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.apply(py)
    }

    /// Shut down every activation and revoke its contributions.
    fn unload<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let activations_store = Arc::clone(&self.activations);
        let linked_store = Arc::clone(&self.linked);
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            unload_activations(&activations_store).await?;
            unload_linked(&linked_store).await?;
            Ok(())
        })
    }

    /// Register an in-process Python extension as a linked package.
    ///
    /// It starts immediately and its contributions are recorded. It is not part
    /// of the installed store or the dependency plan.
    #[pyo3(signature = (manifest, root, start, invoke, shutdown = None))]
    fn register_extension<'py>(
        &self,
        py: Python<'py>,
        manifest: String,
        root: String,
        start: Py<PyAny>,
        invoke: Py<PyAny>,
        shutdown: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let manifest = ExtensionManifest::parse(&manifest).map_err(host_error)?;
        let extension_id = manifest.id().as_str().to_string();
        let extension = PyForeignExtension {
            manifest,
            start,
            invoke,
            shutdown,
        };
        let runtime =
            Arc::new(ExtensionRuntime::new(extension, PathBuf::from(root)).map_err(host_error)?);
        let sink = Arc::clone(&self.sink);
        let linked_store = Arc::clone(&self.linked);
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let contributions = runtime.start().await.map_err(host_error)?;
            let mut agent = ();
            let mut registry = ();
            let lease = match sink
                .apply(
                    &extension_id,
                    contributions.clone(),
                    &mut agent,
                    &mut registry,
                )
                .await
            {
                Ok(lease) => lease,
                Err(error) => {
                    let _ = runtime.shutdown().await;
                    return Err(host_error(error));
                }
            };
            let previous = {
                let mut linked = linked_store
                    .lock()
                    .map_err(|_| host_error("linked store is poisoned"))?;
                linked
                    .iter()
                    .position(|activation| activation.extension == extension_id)
                    .map(|position| linked.remove(position))
            };
            if let Some(previous) = previous {
                let mut lease_host = NoopLeaseHost;
                let _ = previous.lease.revoke(&mut lease_host).await;
                let _ = previous.runtime.shutdown().await;
            }
            if let Ok(mut linked) = linked_store.lock() {
                linked.push(PyLinkedActivation {
                    extension: extension_id,
                    runtime,
                    lease,
                });
            }
            Python::attach(|py| {
                let value = serde_json::to_value(contributions).unwrap_or(serde_json::Value::Null);
                json_to_py(py, &value)
            })
        })
    }

    /// Invoke one contribution on a started extension.
    fn invoke<'py>(
        &self,
        py: Python<'py>,
        extension: String,
        kind: String,
        id: String,
        input: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let input = py_to_json(input)?;
        let linked_runtime = {
            let linked = self
                .linked
                .lock()
                .map_err(|_| host_error("linked store is poisoned"))?;
            linked
                .iter()
                .find(|activation| activation.extension == extension)
                .map(|activation| Arc::clone(&activation.runtime) as Arc<dyn Runtime>)
        };
        let runtime = match linked_runtime {
            Some(runtime) => runtime,
            None => {
                let activations = self
                    .activations
                    .lock()
                    .map_err(|_| host_error("activation store is poisoned"))?;
                activations
                    .iter()
                    .find(|activation| activation.extension == extension)
                    .map(|activation| Arc::clone(&activation.runtime))
                    .ok_or_else(|| host_error(format!("{extension} is not started")))?
            }
        };
        let current_locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        pyo3_async_runtimes::tokio::future_into_py_with_locals(py, current_locals, async move {
            let value = runtime
                .invoke(
                    &ContributionRef { kind, id },
                    input,
                    &InvokeContext {
                        correlation: "binding-call".to_string(),
                        timeout_ms: 30_000,
                    },
                )
                .await
                .map_err(host_error)?;
            Python::attach(|py| json_to_py(py, &value))
        })
    }

    /// Every applied contribution, keyed by extension id.
    fn contributions(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &self.book.to_json())
    }

    /// Grant one capability to an extension.
    #[pyo3(signature = (extension, capability, scope = None, note = None))]
    fn grant(
        &self,
        extension: String,
        capability: String,
        scope: Option<String>,
        note: Option<String>,
    ) -> PyResult<()> {
        let capability = parse_capability(&capability).map_err(host_error)?;
        let scope = parse_scope(scope.as_deref()).map_err(host_error)?;
        self.policy
            .grant(&extension, capability, scope, note)
            .map_err(host_error)
    }

    /// Revoke one capability grant.
    fn revoke_grant(&self, extension: String, capability: String) -> PyResult<bool> {
        let capability = parse_capability(&capability).map_err(host_error)?;
        self.policy
            .revoke(&extension, capability)
            .map_err(host_error)
    }

    /// Every grant for one extension, as a dictionary.
    fn grants(&self, py: Python<'_>, extension: String) -> PyResult<Py<PyAny>> {
        let value = serde_json::to_value(self.policy.grants_for(&extension))
            .unwrap_or(serde_json::Value::Null);
        json_to_py(py, &value)
    }

    /// The first audit persistence failure, if any.
    fn audit_degraded(&self) -> Option<String> {
        self.policy.audit_degraded()
    }

    /// Subscribe to host events. The callback receives one event dictionary.
    fn subscribe_events(&self, topics: Vec<String>, callback: Py<PyAny>) -> PyResult<u32> {
        let filter = EventFilter::from_topics(&topics).map_err(host_error)?;
        let subscription = self.events.subscribe(filter, 128).map_err(host_error)?;
        let id = self.next_subscription.fetch_add(1, Ordering::Relaxed);
        let handle = event_runtime().spawn(async move {
            while let Ok(event) = subscription.recv().await {
                let payload = serde_json::to_value(&event).unwrap_or(serde_json::Value::Null);
                let _ = call_python(&callback, payload).await;
            }
        });
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            subscriptions.push((id, handle));
        }
        Ok(id)
    }

    /// Cancel one event subscription.
    fn unsubscribe_events(&self, id: u32) -> bool {
        let mut subscriptions = match self.subscriptions.lock() {
            Ok(subscriptions) => subscriptions,
            Err(_) => return false,
        };
        let Some(position) = subscriptions
            .iter()
            .position(|(subscription, _)| *subscription == id)
        else {
            return false;
        };
        let (_, handle) = subscriptions.remove(position);
        handle.abort();
        true
    }

    /// Publish one host event to subscribed extensions.
    fn publish_event(
        &self,
        py: Python<'_>,
        topic: String,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let event = ferris_sdk::events::ExtensionEvent::new("binding", topic, py_to_json(payload)?)
            .map_err(host_error)?;
        let report = self.events.publish(&event).map_err(host_error)?;
        json_to_py(
            py,
            &serde_json::json!({
                "delivered": report.delivered,
                "dropped": report.dropped,
            }),
        )
    }
}

async fn unload_linked(store: &StdMutex<Vec<PyLinkedActivation>>) -> PyResult<()> {
    let linked = {
        let mut slot = store
            .lock()
            .map_err(|_| host_error("linked store is poisoned"))?;
        std::mem::take(&mut *slot)
    };
    let mut first = None;
    let mut lease_host = NoopLeaseHost;
    for activation in linked {
        if let Err(error) = activation.lease.revoke(&mut lease_host).await {
            if first.is_none() {
                first = Some(error);
            }
        }
        if let Err(error) = activation.runtime.shutdown().await {
            if first.is_none() {
                first = Some(error);
            }
        }
    }
    first.map_or(Ok(()), |error| Err(host_error(error)))
}

async fn unload_activations(store: &StdMutex<Vec<Activation>>) -> PyResult<()> {
    let activations = {
        let mut slot = store
            .lock()
            .map_err(|_| host_error("activation store is poisoned"))?;
        std::mem::take(&mut *slot)
    };
    let mut first = None;
    let mut lease_host = NoopLeaseHost;
    for activation in activations {
        if let Err(error) = activation.unload(&mut lease_host).await {
            if first.is_none() {
                first = Some(error);
            }
        }
    }
    first.map_or(Ok(()), |error| Err(host_error(error)))
}
