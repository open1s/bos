//! Full Ferris host binding: package store, activation and standard runtimes.
//!
//! This is the host surface, not the authoring harness. It installs packages
//! into a canonical Ferris store, plans them in dependency order, activates the
//! standard runtimes, exposes their contributions, and forwards invocations.
//! The default build enables it; build with `--no-default-features` for the
//! authoring-only addon.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use crate::ferris::{call_js, JsCall, JsExtension, SharedJsCall};

use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi_derive::napi;

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

fn host_error(error: impl std::fmt::Display) -> Error {
  Error::new(napi::Status::GenericFailure, error.to_string())
}

/// Forward contribution lifecycle events to JavaScript.
struct JsContributionCallback {
  on_apply: Option<SharedJsCall>,
  on_revoke: Option<SharedJsCall>,
}

#[async_trait]
impl ContributionCallback for JsContributionCallback {
  async fn applied(
    &self,
    extension: &str,
    contributions: &[ferris_sdk::Contribution],
  ) -> std::result::Result<(), String> {
    if let Some(callback) = &self.on_apply {
      call_js(
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
  ) -> std::result::Result<(), String> {
    if let Some(callback) = &self.on_revoke {
      call_js(
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
///
/// The host owns one canonical store rooted at `root`, installs packages into
/// it, plans them in dependency order, and activates the standard runtimes.
/// Contributions are exposed as JSON and invocations are forwarded to the
/// owning runtime.
#[napi]
pub struct NativeFerrisHost {
  root: PathBuf,
  manager: ExtensionManager,
  policy: Arc<PolicyEngine>,
  events: Arc<EventBus>,
  book: Arc<AppliedContributions>,
  sink: Arc<HostContributionSink>,
  activations: StdMutex<Vec<Activation>>,
  linked: StdMutex<Vec<LinkedActivation>>,
  subscriptions: SubscriptionSlot,
  next_subscription: Arc<AtomicU32>,
}

/// One extension implemented in JavaScript and hosted by this process.
struct LinkedActivation {
  extension: String,
  runtime: Arc<ExtensionRuntime<JsExtension>>,
  lease: Box<dyn ContributionLease>,
}

impl Drop for NativeFerrisHost {
  fn drop(&mut self) {
    if let Ok(subs) = self.subscriptions.lock() {
      for (_, handle) in subs.iter() {
        handle.abort();
      }
    }
  }
}

#[napi]
impl NativeFerrisHost {
  #[napi(constructor)]
  /// Open a host over `root`.
  pub fn new(
    root: String,
    workspace: Option<String>,
    trust: Option<String>,
    policy_mode: Option<String>,
    on_contributions: Option<JsCall>,
    on_revoke: Option<JsCall>,
    process_sandbox: Option<bool>,
  ) -> Result<Self> {
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
      Some(Arc::new(JsContributionCallback {
        on_apply: on_contributions.map(Arc::new),
        on_revoke: on_revoke.map(Arc::new),
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
      manager,
      policy,
      events,
      book,
      sink,
      activations: StdMutex::new(Vec::new()),
      linked: StdMutex::new(Vec::new()),
      subscriptions: Arc::new(StdMutex::new(Vec::new())),
      next_subscription: Arc::new(AtomicU32::new(1)),
    })
  }

  #[napi(getter)]
  /// The canonical store root.
  pub fn root(&self) -> String {
    self.root.to_string_lossy().to_string()
  }

  #[napi]
  /// Installed id/version pairs, sorted by id and version.
  pub fn installed(&self) -> Result<Vec<serde_json::Value>> {
    self
      .manager
      .installed()
      .map(|entries| {
        entries
          .into_iter()
          .map(|entry| serde_json::json!({ "id": entry.id, "version": entry.version }))
          .collect()
      })
      .map_err(host_error)
  }

  #[napi]
  /// Install one package tree and return its identity.
  pub fn install(&self, source: String) -> Result<serde_json::Value> {
    let installed = self
      .manager
      .install(&PathBuf::from(source))
      .map_err(host_error)?;
    Ok(serde_json::json!({ "id": installed.id, "version": installed.version }))
  }

  #[napi]
  /// Install one package tree with an explicit source label.
  pub fn install_package(
    &self,
    source: String,
    origin: Option<String>,
  ) -> Result<serde_json::Value> {
    let origin = match origin.unwrap_or_else(|| "installed".to_string()).as_str() {
      "development" | "dev" => ExtensionSource::Development,
      "builtin" => ExtensionSource::Builtin,
      _ => ExtensionSource::Installed,
    };
    let package = self
      .manager
      .install_package(&PathBuf::from(source), origin)
      .map_err(host_error)?;
    Ok(serde_json::json!({
        "id": package.id().as_str(),
        "version": package.version(),
    }))
  }

  #[napi]
  /// Remove one installed extension, optionally purging its data.
  pub fn uninstall(&self, id: String, purge_data: Option<bool>) -> Result<()> {
    self
      .manager
      .uninstall(&id, purge_data.unwrap_or(false))
      .map_err(host_error)
  }

  #[napi]
  /// Enable or disable one installed extension.
  pub fn set_enabled(&self, id: String, enabled: bool) -> Result<()> {
    self.manager.set_enabled(&id, enabled).map_err(host_error)
  }

  #[napi]
  /// Every installed version of one extension, ascending.
  pub fn versions(&self, id: String) -> Result<Vec<String>> {
    self.manager.versions(&id).map_err(host_error)
  }

  #[napi]
  /// The active version of one extension.
  pub fn active_version(&self, id: String) -> Result<Option<String>> {
    self.manager.active_version(&id).map_err(host_error)
  }

  #[napi]
  /// Activate one installed version.
  pub fn activate(&self, id: String, version: String) -> Result<()> {
    self.manager.activate(&id, &version).map_err(host_error)
  }

  #[napi]
  /// Activate the version before the current one.
  pub fn rollback(&self, id: String) -> Result<String> {
    self.manager.rollback(&id).map_err(host_error)
  }

  #[napi]
  /// Clear a persisted failure for one extension.
  pub fn clear_failure(&self, id: String) -> Result<()> {
    self.manager.clear_failure(&id).map_err(host_error)
  }

  #[napi]
  /// Move one extension's code to quarantine.
  pub fn quarantine(&self, id: String, reason: String) -> Result<String> {
    self
      .manager
      .quarantine(&id, &reason)
      .map(|path| path.to_string_lossy().to_string())
      .map_err(host_error)
  }

  #[napi]
  /// The canonical package root for one installed version.
  pub fn package_root(&self, id: String, version: String) -> Result<String> {
    self
      .manager
      .package_root(&id, &version)
      .map(|path| path.to_string_lossy().to_string())
      .map_err(host_error)
  }

  #[napi]
  /// Resolve installed packages into an activation order.
  pub fn plan(&self) -> Result<Vec<String>> {
    self
      .manager
      .plan()
      .map(|plan| plan.order)
      .map_err(host_error)
  }

  #[napi]
  /// Start every planned package and return the applied contributions.
  pub async fn apply(&self) -> Result<serde_json::Value> {
    // Plan first: a broken package graph must not tear down the current set.
    let plan = self.manager.plan().map_err(host_error)?;
    self.unload_packages().await?;
    let order = plan.order.clone();
    let mut agent_context = ();
    let mut registry_context = ();
    let mut lease_host = NoopLeaseHost;
    let mut activation_set = self
      .manager
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
    if let Ok(mut activations) = self.activations.lock() {
      activations.extend(started);
    }
    Ok(self.contributions())
  }

  #[napi]
  /// Re-apply the current plan (unload, then start again).
  pub async fn reload(&self) -> Result<serde_json::Value> {
    self.apply().await
  }

  #[napi]
  /// Shut down every activation and revoke its contributions.
  pub async fn unload(&self) -> Result<()> {
    self.unload_inner().await
  }

  async fn unload_packages(&self) -> Result<()> {
    let activations = {
      let mut slot = self.activations.lock().map_err(host_error)?;
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

  async fn unload_inner(&self) -> Result<()> {
    self.unload_packages().await?;
    let linked = {
      let mut slot = self.linked.lock().map_err(host_error)?;
      std::mem::take(&mut *slot)
    };
    let mut first = None;
    let mut lease_host = NoopLeaseHost;
    for linked in linked {
      if let Err(error) = linked.lease.revoke(&mut lease_host).await {
        if first.is_none() {
          first = Some(error);
        }
      }
      if let Err(error) = linked.runtime.shutdown().await {
        if first.is_none() {
          first = Some(error);
        }
      }
    }
    first.map_or(Ok(()), |error| Err(host_error(error)))
  }

  #[napi]
  /// Register an in-process JavaScript extension as a linked package.
  ///
  /// The extension starts immediately and its contributions are recorded. It
  /// is not part of the installed store or the dependency plan.
  pub async fn register_extension(
    &self,
    manifest: String,
    root: String,
    start: JsCall,
    invoke: JsCall,
    shutdown: Option<JsCall>,
  ) -> Result<serde_json::Value> {
    let manifest = ExtensionManifest::parse(&manifest).map_err(host_error)?;
    let extension_id = manifest.id().as_str().to_string();
    let extension = JsExtension {
      manifest,
      start: Arc::new(start),
      invoke: Arc::new(invoke),
      shutdown: shutdown.map(Arc::new),
    };
    let runtime =
      Arc::new(ExtensionRuntime::new(extension, PathBuf::from(root)).map_err(host_error)?);
    let contributions = runtime.start().await.map_err(host_error)?;
    let mut agent = ();
    let mut registry = ();
    let lease = match self
      .sink
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
        // The runtime already started; a failed contribution must not leak it.
        let _ = runtime.shutdown().await;
        return Err(host_error(error));
      }
    };
    let previous = {
      let mut linked = self.linked.lock().map_err(host_error)?;
      linked
        .iter()
        .position(|activation| activation.extension == extension_id)
        .map(|position| linked.remove(position))
    };
    if let Some(previous) = previous {
      // Replacing a registration must tear the old one down, not drop it.
      let mut lease_host = NoopLeaseHost;
      let _ = previous.lease.revoke(&mut lease_host).await;
      let _ = previous.runtime.shutdown().await;
    }
    let mut linked = self.linked.lock().map_err(host_error)?;
    linked.push(LinkedActivation {
      extension: extension_id,
      runtime,
      lease,
    });
    serde_json::to_value(contributions)
      .map_err(|error| Error::new(napi::Status::GenericFailure, error.to_string()))
  }

  #[napi]
  /// Invoke one contribution on a started extension.
  pub async fn invoke(
    &self,
    extension: String,
    kind: String,
    id: String,
    input: serde_json::Value,
  ) -> Result<serde_json::Value> {
    let linked_runtime = {
      let linked = self.linked.lock().map_err(host_error)?;
      linked
        .iter()
        .find(|activation| activation.extension == extension)
        .map(|activation| Arc::clone(&activation.runtime) as Arc<dyn Runtime>)
    };
    let runtime = match linked_runtime {
      Some(runtime) => runtime,
      None => {
        let activations = self.activations.lock().map_err(host_error)?;
        activations
          .iter()
          .find(|activation| activation.extension == extension)
          .map(|activation| Arc::clone(&activation.runtime))
          .ok_or_else(|| host_error(format!("{extension} is not started")))?
      }
    };
    runtime
      .invoke(
        &ContributionRef { kind, id },
        input,
        &InvokeContext {
          correlation: "binding-call".to_string(),
          timeout_ms: 30_000,
        },
      )
      .await
      .map_err(host_error)
  }

  #[napi]
  /// Every applied contribution, keyed by extension id.
  pub fn contributions(&self) -> serde_json::Value {
    self.book.to_json()
  }

  #[napi]
  /// Grant one capability to an extension.
  pub fn grant(
    &self,
    extension: String,
    capability: String,
    scope: Option<String>,
    note: Option<String>,
  ) -> Result<()> {
    let capability = parse_capability(&capability).map_err(host_error)?;
    let scope = parse_scope(scope.as_deref()).map_err(host_error)?;
    self
      .policy
      .grant(&extension, capability, scope, note)
      .map_err(host_error)
  }

  #[napi]
  /// Revoke one capability grant.
  pub fn revoke_grant(&self, extension: String, capability: String) -> Result<bool> {
    let capability = parse_capability(&capability).map_err(host_error)?;
    self
      .policy
      .revoke(&extension, capability)
      .map_err(host_error)
  }

  #[napi]
  /// Every grant for one extension, as JSON.
  pub fn grants(&self, extension: String) -> serde_json::Value {
    serde_json::to_value(self.policy.grants_for(&extension)).unwrap_or(serde_json::Value::Null)
  }

  #[napi]
  /// The first audit persistence failure, if any.
  pub fn audit_degraded(&self) -> Option<String> {
    self.policy.audit_degraded()
  }

  #[napi]
  /// Subscribe to host events. The callback receives one event object.
  pub fn subscribe_events(&self, topics: Vec<String>, callback: JsCall) -> Result<u32> {
    let filter = EventFilter::from_topics(&topics).map_err(host_error)?;
    let subscription = self.events.subscribe(filter, 128).map_err(host_error)?;
    let id = self.next_subscription.fetch_add(1, Ordering::Relaxed);
    let callback = Arc::new(callback);
    let handle = event_runtime().spawn(async move {
      while let Ok(event) = subscription.recv().await {
        let payload = serde_json::to_value(&event).unwrap_or(serde_json::Value::Null);
        let _ = call_js(&callback, payload).await;
      }
    });
    if let Ok(mut subscriptions) = self.subscriptions.lock() {
      subscriptions.push((id, handle));
    }
    Ok(id)
  }

  #[napi]
  /// Cancel one event subscription.
  pub fn unsubscribe_events(&self, id: u32) -> bool {
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

  #[napi]
  /// Publish one host event to subscribed extensions.
  pub fn publish_event(
    &self,
    topic: String,
    payload: serde_json::Value,
  ) -> Result<serde_json::Value> {
    let event =
      ferris_sdk::events::ExtensionEvent::new("binding", topic, payload).map_err(host_error)?;
    let report = self.events.publish(&event).map_err(host_error)?;
    Ok(serde_json::json!({
        "delivered": report.delivered,
        "dropped": report.dropped,
    }))
  }
}
