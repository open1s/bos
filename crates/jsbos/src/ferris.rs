//! Node.js bindings for the Ferris SDK authoring surface.
//!
//! A JavaScript object can implement the SDK's `Extension` trait: it declares a
//! schema-2.0 manifest and supplies `start`, `invoke` and optional `shutdown`
//! callbacks. `FerrisHarness` adapts those callbacks to the Rust
//! `ExtensionRuntime`, so an extension written in JavaScript starts and answers
//! calls exactly like one written in Rust.

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::Unknown;
use napi_derive::napi;

use ferris_sdk::{
  Call, Contribution, Extension, ExtensionContext, ExtensionManifest, ExtensionRuntime,
  InvokeContext, Runtime,
};

use crate::jsany::JSAny;

/// A JavaScript function taking one JSON value and returning one.
pub(crate) type JsCall =
  ThreadsafeFunction<JSAny, Unknown<'static>, JSAny, napi::Status, true, true>;
/// A shared handle to one JavaScript callback.
pub(crate) type SharedJsCall = Arc<JsCall>;

pub(crate) fn ferris_error(error: ferris_sdk::ExtError) -> Error {
  Error::new(napi::Status::GenericFailure, error.to_string())
}

fn extract_json_value(val: Unknown<'_>) -> napi::Result<serde_json::Value> {
  // A callback that returns nothing (the usual shutdown handler) is a null
  // result, not a conversion failure.
  if matches!(
    val.get_type(),
    Ok(napi::ValueType::Undefined) | Ok(napi::ValueType::Null)
  ) {
    return Ok(serde_json::Value::Null);
  }
  let raw = val.value();
  unsafe {
    let env = raw.env;
    let napi_val = raw.value;
    <serde_json::Value as napi::bindgen_prelude::FromNapiValue>::from_napi_value(env, napi_val)
  }
}

/// Call one JavaScript callback, awaiting a returned promise.
pub(crate) async fn call_js(
  callback: &SharedJsCall,
  input: serde_json::Value,
) -> std::result::Result<serde_json::Value, String> {
  use napi::bindgen_prelude::{CallbackContext, PromiseRaw};

  let (tx, rx) = tokio::sync::oneshot::channel::<std::result::Result<serde_json::Value, String>>();
  let tx = Arc::new(StdMutex::new(Some(tx)));

  callback.call_with_return_value(
    Ok(JSAny(input)),
    ThreadsafeFunctionCallMode::NonBlocking,
    move |result: std::result::Result<Unknown<'static>, napi::Error>, env| -> napi::Result<()> {
      match result {
        Ok(val) => {
          if val.is_promise().unwrap_or(false) {
            let raw_env = env.raw();
            let raw_val = val.value().value;
            let promise_raw = PromiseRaw::<Unknown<'static>>::new(raw_env, raw_val);
            let tx_then = tx.clone();
            let tx_catch = tx.clone();
            match promise_raw.then(move |ctx: CallbackContext<Unknown<'static>>| {
              let outcome = extract_json_value(ctx.value).map_err(|error| error.to_string());
              if let Some(tx) = tx_then.lock().unwrap().take() {
                let _ = tx.send(outcome);
              }
              Ok(())
            }) {
              Ok(chained) => {
                // A rejected promise never reaches then; without catch the
                // channel would stay open and every failed callback would hang
                // the caller instead of surfacing the error.
                let _ = chained.catch(move |ctx: CallbackContext<Unknown<'static>>| {
                  let message = extract_json_value(ctx.value)
                    .map(|value| value.to_string())
                    .unwrap_or_else(|error| error.to_string());
                  if let Some(tx) = tx_catch.lock().unwrap().take() {
                    let _ = tx.send(Err(message));
                  }
                  Ok(())
                });
              }
              Err(error) => {
                if let Some(tx) = tx_catch.lock().unwrap().take() {
                  let _ = tx.send(Err(error.to_string()));
                }
              }
            }
          } else {
            let outcome = extract_json_value(val).map_err(|error| error.to_string());
            if let Some(tx) = tx.lock().unwrap().take() {
              let _ = tx.send(outcome);
            }
          }
        }
        Err(error) => {
          if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(Err(error.to_string()));
          }
        }
      }
      Ok(())
    },
  );

  rx.await
    .unwrap_or_else(|_| Err("callback channel closed".to_string()))
}

/// One extension implemented by JavaScript callbacks.
pub(crate) struct JsExtension {
  pub(crate) manifest: ExtensionManifest,
  pub(crate) start: SharedJsCall,
  pub(crate) invoke: SharedJsCall,
  pub(crate) shutdown: Option<SharedJsCall>,
}

#[async_trait]
impl Extension for JsExtension {
  fn manifest(&self) -> ExtensionManifest {
    self.manifest.clone()
  }

  async fn start(&self, ctx: &ExtensionContext) -> ferris_sdk::Result<Vec<Contribution>> {
    let payload = serde_json::json!({
        "id": ctx.id(),
        "root": ctx.root().to_string_lossy(),
        "data_dir": ctx.data_dir().to_string_lossy(),
        "runtime_kind": ctx.manifest.runtime_kind(),
    });
    let value = call_js(&self.start, payload)
      .await
      .map_err(ferris_sdk::error::runtime)?;
    serde_json::from_value(value)
      .map_err(|error| ferris_sdk::error::invalid_input(error.to_string()))
  }

  async fn invoke(
    &self,
    call: &Call,
    ctx: &InvokeContext,
  ) -> ferris_sdk::Result<serde_json::Value> {
    let payload = serde_json::json!({
        "contribution": { "kind": call.kind(), "id": call.id() },
        "input": call.input,
        "context": { "correlation": ctx.correlation, "timeout_ms": ctx.timeout_ms },
    });
    call_js(&self.invoke, payload)
      .await
      .map_err(ferris_sdk::error::runtime)
  }

  async fn shutdown(&self) -> ferris_sdk::Result<()> {
    if let Some(callback) = &self.shutdown {
      call_js(callback, serde_json::json!({}))
        .await
        .map_err(ferris_sdk::error::runtime)?;
    }
    Ok(())
  }
}

/// A parsed, validated schema-2.0 extension manifest.
#[napi]
pub struct FerrisManifest {
  inner: ExtensionManifest,
}

impl FerrisManifest {
  pub(crate) fn from_manifest(inner: ExtensionManifest) -> Self {
    Self { inner }
  }
}

#[napi]
impl FerrisManifest {
  #[napi(factory)]
  /// Parse and validate a `extension.toml` document.
  pub fn parse(text: String) -> Result<Self> {
    let inner = ExtensionManifest::parse(&text).map_err(ferris_error)?;
    Ok(Self { inner })
  }

  #[napi]
  /// The canonical TOML document.
  pub fn to_toml(&self) -> String {
    self.inner.to_toml().to_string()
  }

  #[napi(js_name = "toJSON")]
  /// The manifest as a plain JSON object.
  pub fn to_json(&self) -> serde_json::Value {
    serde_json::to_value(self.inner.as_v2()).unwrap_or(serde_json::Value::Null)
  }

  #[napi(getter)]
  /// The extension id.
  pub fn id(&self) -> String {
    self.inner.id().as_str().to_string()
  }

  #[napi(getter)]
  /// The runtime kind the manifest asks for.
  pub fn runtime_kind(&self) -> String {
    self.inner.runtime_kind().to_string()
  }
}

/// An in-process Ferris extension harness.
///
/// This is the JavaScript equivalent of the SDK's `TestHarness`: it starts the
/// extension, returns its contributions, and forwards `invoke` calls. It does
/// not install or activate packages; that is the host runtime surface.
#[napi]
pub struct NativeFerrisHarness {
  inner: Arc<ExtensionRuntime<JsExtension>>,
}

#[napi]
impl NativeFerrisHarness {
  #[napi(constructor)]
  /// Build a harness around JavaScript extension callbacks.
  ///
  /// `start(ctx)` receives the start context and returns an array of
  /// `{ kind, id, payload }` contributions. `invoke(call)` receives
  /// `{ contribution, input, context }` and returns the call result.
  pub fn new(
    manifest: String,
    root: String,
    start: JsCall,
    invoke: JsCall,
    shutdown: Option<JsCall>,
  ) -> Result<Self> {
    let manifest = ExtensionManifest::parse(&manifest).map_err(ferris_error)?;
    let extension = JsExtension {
      manifest,
      start: Arc::new(start),
      invoke: Arc::new(invoke),
      shutdown: shutdown.map(Arc::new),
    };
    let runtime = ExtensionRuntime::new(extension, PathBuf::from(root)).map_err(ferris_error)?;
    Ok(Self {
      inner: Arc::new(runtime),
    })
  }

  #[napi]
  /// Start the extension and return its contributions.
  pub async fn start(&self) -> Result<serde_json::Value> {
    let inner = self.inner.clone();
    let contributions = inner.start().await.map_err(ferris_error)?;
    serde_json::to_value(contributions)
      .map_err(|error| Error::new(napi::Status::GenericFailure, error.to_string()))
  }

  #[napi]
  /// Invoke a contribution id as a tool.
  pub async fn invoke(&self, id: String, input: serde_json::Value) -> Result<serde_json::Value> {
    let inner = self.inner.clone();
    inner.invoke_id(&id, input).await.map_err(ferris_error)
  }

  #[napi]
  /// Invoke a contribution with an explicit kind.
  pub async fn invoke_contribution(
    &self,
    kind: String,
    id: String,
    input: serde_json::Value,
  ) -> Result<serde_json::Value> {
    let inner = self.inner.clone();
    inner
      .invoke_contribution(&kind, &id, input)
      .await
      .map_err(ferris_error)
  }

  #[napi]
  /// Shut the extension down.
  pub async fn shutdown(&self) -> Result<()> {
    let inner = self.inner.clone();
    inner.shutdown().await.map_err(ferris_error)
  }

  #[napi]
  /// The manifest as a plain JSON object.
  pub fn manifest(&self) -> serde_json::Value {
    serde_json::to_value(self.inner.manifest().as_v2()).unwrap_or(serde_json::Value::Null)
  }

  #[napi(getter)]
  /// The runtime kind the manifest asks for.
  pub fn runtime_kind(&self) -> String {
    self.inner.manifest().runtime_kind().to_string()
  }

  #[napi(getter)]
  /// The package root handed to the extension.
  pub fn root(&self) -> String {
    self.inner.root().to_string_lossy().to_string()
  }
}

/// Convert a JSON value into a TOML value for runtime configuration and
/// contribution payloads. JSON null has no TOML representation.
pub(crate) fn json_to_toml(
  value: &serde_json::Value,
) -> std::result::Result<ferris_sdk::toml::Value, String> {
  ferris_bindings::json_to_toml(value)
}

fn startup_policy(raw: &str) -> Result<ferris_sdk::StartupPolicy> {
  ferris_bindings::startup_policy(raw).map_err(|error| Error::new(napi::Status::InvalidArg, error))
}

/// A chainable builder for a schema-2.0 manifest.
///
/// Every method returns a new builder, so calls compose in JavaScript and
/// Python without a second native object per step.
#[napi]
pub struct FerrisManifestBuilder {
  inner: ferris_sdk::ManifestBuilder,
}

#[napi]
impl FerrisManifestBuilder {
  #[napi(constructor)]
  /// Start a manifest with the required identity fields.
  pub fn new(id: String, name: String, version: String, description: String) -> Self {
    Self {
      inner: ferris_sdk::ManifestBuilder::new(id, name, version, description),
    }
  }

  #[napi]
  /// Set the host version range.
  pub fn host(&self, requirement: String) -> Self {
    Self {
      inner: self.inner.clone().host(requirement),
    }
  }

  #[napi]
  /// Set the extension API range.
  pub fn extension_api(&self, requirement: String) -> Self {
    Self {
      inner: self.inner.clone().extension_api(requirement),
    }
  }

  #[napi]
  /// Set the runtime kind.
  pub fn runtime(&self, kind: String) -> Self {
    Self {
      inner: self.inner.clone().runtime(kind),
    }
  }

  #[napi]
  /// Set one opaque runtime configuration value.
  pub fn runtime_config(&self, key: String, value: serde_json::Value) -> Result<Self> {
    let value = json_to_toml(&value).map_err(|error| {
      Error::new(
        napi::Status::InvalidArg,
        format!("invalid runtime config: {error}"),
      )
    })?;
    Ok(Self {
      inner: self.inner.clone().runtime_config(key, value),
    })
  }

  #[napi]
  /// Require one capability.
  pub fn capability(&self, capability: String) -> Self {
    Self {
      inner: self.inner.clone().capability(capability),
    }
  }

  #[napi]
  /// Declare one contribution with an optional JSON payload.
  pub fn contribution(
    &self,
    kind: String,
    id: String,
    payload: Option<serde_json::Value>,
  ) -> Result<Self> {
    let mut spec = ferris_sdk::ContributionSpec::new(kind, id);
    if let Some(payload) = payload {
      let payload = json_to_toml(&payload).map_err(|error| {
        Error::new(
          napi::Status::InvalidArg,
          format!("invalid contribution payload: {error}"),
        )
      })?;
      spec = spec.payload(payload);
    }
    Ok(Self {
      inner: self.inner.clone().contribution(spec),
    })
  }

  #[napi]
  /// Add one author.
  pub fn author(&self, author: String) -> Self {
    Self {
      inner: self.inner.clone().author(author),
    }
  }

  #[napi]
  /// Set the SPDX license expression.
  pub fn license(&self, license: String) -> Self {
    Self {
      inner: self.inner.clone().license(license),
    }
  }

  #[napi]
  /// Set the source repository URL.
  pub fn repository(&self, repository: String) -> Self {
    Self {
      inner: self.inner.clone().repository(repository),
    }
  }

  #[napi]
  /// Set the project homepage URL.
  pub fn homepage(&self, homepage: String) -> Self {
    Self {
      inner: self.inner.clone().homepage(homepage),
    }
  }

  #[napi]
  /// Add one dependency requirement.
  pub fn dependency(&self, id: String, requirement: String) -> Self {
    Self {
      inner: self.inner.clone().dependency(id, requirement),
    }
  }

  #[napi]
  /// Declare one provided service from a JSON spec.
  pub fn provide(&self, spec: serde_json::Value) -> Result<Self> {
    let spec: ferris_sdk::ServiceProvideSpec = serde_json::from_value(spec).map_err(|error| {
      Error::new(
        napi::Status::InvalidArg,
        format!("invalid service provide spec: {error}"),
      )
    })?;
    Ok(Self {
      inner: self.inner.clone().provide(spec),
    })
  }

  #[napi]
  /// Declare one required service from a JSON spec.
  pub fn require(&self, spec: serde_json::Value) -> Result<Self> {
    let spec: ferris_sdk::ServiceRequireSpec = serde_json::from_value(spec).map_err(|error| {
      Error::new(
        napi::Status::InvalidArg,
        format!("invalid service require spec: {error}"),
      )
    })?;
    Ok(Self {
      inner: self.inner.clone().require(spec),
    })
  }

  #[napi]
  /// Set the memory ceiling in MiB.
  pub fn max_memory_mb(&self, value: u32) -> Self {
    Self {
      inner: self.inner.clone().max_memory_mb(value as u64),
    }
  }

  #[napi]
  /// Set the one-call wall-clock ceiling in milliseconds.
  pub fn max_execution_ms(&self, value: u32) -> Self {
    Self {
      inner: self.inner.clone().max_execution_ms(value as u64),
    }
  }

  #[napi]
  /// Set the concurrent-call ceiling.
  pub fn max_concurrent_calls(&self, value: u32) -> Self {
    Self {
      inner: self.inner.clone().max_concurrent_calls(value),
    }
  }

  #[napi]
  /// Set the per-result output ceiling in bytes.
  pub fn max_output_bytes(&self, value: u32) -> Self {
    Self {
      inner: self.inner.clone().max_output_bytes(value as u64),
    }
  }

  #[napi]
  /// Set the WASM table element ceiling.
  pub fn max_table_elements(&self, value: u32) -> Self {
    Self {
      inner: self.inner.clone().max_table_elements(value as u64),
    }
  }

  #[napi]
  /// Set the package-relative configuration schema path.
  pub fn configuration_schema(&self, path: String) -> Self {
    Self {
      inner: self.inner.clone().configuration_schema(path),
    }
  }

  #[napi]
  /// Set the default configuration values.
  pub fn configuration_defaults(&self, value: serde_json::Value) -> Result<Self> {
    let value = json_to_toml(&value).map_err(|error| {
      Error::new(
        napi::Status::InvalidArg,
        format!("invalid configuration defaults: {error}"),
      )
    })?;
    Ok(Self {
      inner: self.inner.clone().configuration_defaults(value),
    })
  }

  #[napi]
  /// Set the startup policy: on-startup, on-demand, on-workspace-open, manual.
  pub fn startup(&self, policy: String) -> Result<Self> {
    Ok(Self {
      inner: self.inner.clone().startup(startup_policy(&policy)?),
    })
  }

  #[napi]
  /// Validate and build the manifest.
  pub fn build(&self) -> Result<FerrisManifest> {
    let inner = self.inner.clone().build().map_err(ferris_error)?;
    Ok(FerrisManifest::from_manifest(inner))
  }
}
