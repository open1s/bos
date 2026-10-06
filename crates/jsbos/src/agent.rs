#![allow(clippy::unnecessary_cast)]

use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::Unknown;
use napi_derive::napi;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

use crate::hooks::{HookContextData, HookEvent, HookRegistry};
use crate::jsany::JSAny;
use agent::BashTool;
use react::llm::{Content, ContentPart};
use react::tool::registry::AsyncTool;

fn extract_json_value(val: Unknown<'_>) -> napi::Result<serde_json::Value> {
  let raw = val.value();
  unsafe {
    let env = raw.env;
    let napi_val = raw.value;
    <serde_json::Value as napi::bindgen_prelude::FromNapiValue>::from_napi_value(env, napi_val)
  }
}

#[napi(object)]
pub struct JsContent {
  #[napi(js_name = "type")]
  pub part_type: String,
  pub text: Option<String>,
  pub content_type: Option<String>,
  pub url: Option<String>,
  pub base64: Option<String>,
  pub name: Option<String>,
}

impl From<JsContent> for ContentPart {
  fn from(js: JsContent) -> Self {
    match js.part_type.as_str() {
      "text" => ContentPart::Text {
        text: js.text.unwrap_or_default(),
      },
      _ => ContentPart::Binary {
        binary: react::llm::Binary {
          content_type: js.content_type.unwrap_or_else(|| "image/jpeg".to_string()),
          source: if let Some(url) = js.url {
            react::llm::BinarySource::Url(url)
          } else {
            react::llm::BinarySource::Base64(js.base64.unwrap_or_default())
          },
          name: js.name,
        },
      },
    }
  }
}

fn vec_jscontent_to_content(parts: Vec<JsContent>) -> Content {
  if parts.is_empty() {
    Content::Text(String::new())
  } else {
    Content::Parts(parts.into_iter().map(Into::into).collect())
  }
}

struct JSTool {
  name: String,
  description: String,
  schema: serde_json::Value,
  cancelable: bool,
  // Weak threadsafe functions so a registered tool does not pin the Node
  // event loop: a script that only registers tools must still be able to exit.
  callback: Arc<ThreadsafeFunction<JSAny, napi::Unknown<'static>, JSAny, napi::Status, true, true>>,
  cancel_callback: Option<
    Arc<ThreadsafeFunction<String, napi::Unknown<'static>, String, napi::Status, true, true>>,
  >,
}

#[async_trait]
impl AsyncTool for JSTool {
  fn name(&self) -> &str {
    &self.name
  }

  fn description(&self) -> String {
    self.description.clone()
  }

  fn json_schema(&self) -> serde_json::Value {
    self.schema.clone()
  }

  fn is_cancelable(&self) -> bool {
    self.cancelable
  }

  fn cancel(&self, call_id: &str) {
    if let Some(cb) = &self.cancel_callback {
      let _ = cb.call(
        Ok(call_id.to_string()),
        ThreadsafeFunctionCallMode::NonBlocking,
      );
    }
  }

  async fn run(
    &self,
    args: &serde_json::Value,
  ) -> std::result::Result<serde_json::Value, react::ToolError> {
    let args_json = args.clone();
    let callback = self.callback.clone();

    let (tx, rx) =
      tokio::sync::oneshot::channel::<std::result::Result<serde_json::Value, String>>();

    // call_with_return_value already runs on a NAPI worker thread,
    // so no need for spawn_blocking.
    callback.call_with_return_value(
      Ok(JSAny(args_json)),
      ThreadsafeFunctionCallMode::NonBlocking,
      move |result: std::result::Result<Unknown<'static>, napi::Error>, env| -> napi::Result<()> {
        match result {
          Ok(val) => {
            let is_promise = val.is_promise().unwrap_or(false);
            if is_promise {
              let raw_env = env.raw();
              let raw_val = val.value().value;
              let promise_raw = PromiseRaw::<Unknown<'static>>::new(raw_env, raw_val);
              let tx = Arc::new(StdMutex::new(Some(tx)));
              let _ = promise_raw.then(move |ctx: CallbackContext<Unknown<'static>>| {
                let json_val = extract_json_value(ctx.value)
                  .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));
                if let Some(tx) = tx.lock().unwrap().take() {
                  let _ = tx.send(Ok(json_val));
                }
                Ok(())
              });
            } else {
              let json_val = extract_json_value(val)
                .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));
              let _ = tx.send(Ok(json_val));
            }
          }
          Err(e) => {
            let _ = tx.send(Err(e.to_string()));
          }
        }
        Ok(())
      },
    );

    match rx.await {
      Ok(Ok(result)) => std::result::Result::Ok(result),
      Ok(Err(e)) => std::result::Result::Err(react::ToolError::Failed(e.to_string())),
      Err(_) => std::result::Result::Err(react::ToolError::Failed(
        "handler channel closed".to_string(),
      )),
    }
  }
}

#[napi(object)]
pub struct StopOptions {
  pub clear_session: Option<bool>,
}

#[napi(object)]
/// Configuration for an agent.
pub struct AgentConfig {
  /// Agent name.
  pub name: String,
  /// Model identifier.
  pub model: String,
  /// LLM API base URL.
  #[napi(js_name = "baseUrl")]
  pub base_url: String,
  /// LLM API key.
  #[napi(js_name = "apiKey")]
  pub api_key: String,
  /// System prompt.
  #[napi(js_name = "systemPrompt")]
  pub system_prompt: String,
  /// Sampling temperature.
  pub temperature: f64,
  /// Completion token cap.
  #[napi(js_name = "maxTokens")]
  pub max_tokens: Option<i32>,
  /// Per-request timeout in seconds.
  #[napi(js_name = "timeoutSecs")]
  pub timeout_secs: i64,
  /// Maximum ReAct steps.
  #[napi(js_name = "maxSteps")]
  pub max_steps: Option<i64>,
  /// API protocol.
  #[napi(js_name = "apiMode")]
  pub api_mode: Option<String>,
  /// Reasoning effort, if any.
  #[napi(js_name = "reasoningEffort")]
  pub reasoning_effort: Option<String>,
  /// Circuit breaker failure threshold.
  #[napi(js_name = "circuitBreakerMaxFailures")]
  pub circuit_breaker_max_failures: Option<i32>,
  /// Circuit breaker cooldown in seconds.
  #[napi(js_name = "circuitBreakerCooldownSecs")]
  pub circuit_breaker_cooldown_secs: Option<i64>,
  /// Rate limiter burst capacity.
  #[napi(js_name = "rateLimitCapacity")]
  pub rate_limit_capacity: Option<i32>,
  /// Rate limiter window in seconds.
  #[napi(js_name = "rateLimitWindowSecs")]
  pub rate_limit_window_secs: Option<i64>,
  /// Maximum retries when rate limited.
  #[napi(js_name = "rateLimitMaxRetries")]
  pub rate_limit_max_retries: Option<i32>,
  /// Retry backoff in seconds.
  #[napi(js_name = "rateLimitRetryBackoffSecs")]
  pub rate_limit_retry_backoff_secs: Option<i64>,
  /// Whether to wait automatically when rate limited.
  #[napi(js_name = "rateLimitAutoWait")]
  pub rate_limit_auto_wait: Option<bool>,
}

impl Default for AgentConfig {
  fn default() -> Self {
    let c = agent::AgentConfig::default();
    Self {
      name: c.name,
      model: c.model,
      base_url: c.base_url,
      api_key: c.api_key,
      system_prompt: c.system_prompt,
      temperature: c.temperature as f64,
      max_tokens: c.max_tokens.map(|v| v as i32),
      timeout_secs: c.timeout_secs as i64,
      max_steps: None,
      api_mode: Some("chat".to_string()),
      reasoning_effort: None,
      circuit_breaker_max_failures: None,
      circuit_breaker_cooldown_secs: None,
      rate_limit_capacity: None,
      rate_limit_window_secs: None,
      rate_limit_max_retries: None,
      rate_limit_retry_backoff_secs: None,
      rate_limit_auto_wait: None,
    }
  }
}

impl From<AgentConfig> for agent::AgentConfig {
  fn from(value: AgentConfig) -> Self {
    // 0 means "disable circuit breaker" - treat as None to disable entirely
    let cb_enabled = value.circuit_breaker_max_failures.unwrap_or(0) > 0
      || value.circuit_breaker_cooldown_secs.is_some();
    let circuit_breaker = if cb_enabled {
      Some(agent::CircuitBreakerConfig {
        max_failures: value.circuit_breaker_max_failures.unwrap_or(5) as usize,
        cooldown: std::time::Duration::from_secs(
          value.circuit_breaker_cooldown_secs.unwrap_or(30) as u64
        ),
      })
    } else {
      None
    };

    let rate_limit = if value.rate_limit_capacity.is_some()
      || value.rate_limit_window_secs.is_some()
      || value.rate_limit_max_retries.is_some()
    {
      Some(agent::RateLimiterConfig {
        capacity: value.rate_limit_capacity.unwrap_or(40) as u32,
        window: std::time::Duration::from_secs(value.rate_limit_window_secs.unwrap_or(60) as u64),
        max_retries: value.rate_limit_max_retries.unwrap_or(3) as u32,
        retry_backoff: std::time::Duration::from_secs(
          value.rate_limit_retry_backoff_secs.unwrap_or(1) as u64,
        ),
        auto_wait: value.rate_limit_auto_wait.unwrap_or(true),
      })
    } else {
      None
    };

    let max_tokens_converted = value.max_tokens.map(|v| v as u32);

    Self {
      name: value.name,
      model: value.model,
      base_url: value.base_url,
      api_key: value.api_key,
      system_prompt: value.system_prompt,
      temperature: value.temperature as f32,
      max_tokens: max_tokens_converted,
      timeout_secs: value.timeout_secs as u64,
      max_steps: value.max_steps.unwrap_or(10) as usize,
      api_mode: value.api_mode.unwrap_or_else(|| "chat".to_string()),
      reasoning_effort: value.reasoning_effort,
      circuit_breaker,
      rate_limit,
    }
  }
}

#[napi]
/// A JavaScript-facing agent.
pub struct Agent {
  inner: Arc<Mutex<agent::Agent>>,
  bus_session: Option<Arc<crate::Session>>,
  #[allow(dead_code)]
  hooks: std::sync::Arc<std::sync::Mutex<HookRegistry>>,
  perf: std::sync::Arc<crate::perf::PerformanceMetrics>,
  stop_flag: std::sync::Arc<AtomicBool>,
  is_running: std::sync::Arc<AtomicBool>,
}

#[napi]
impl Agent {
  #[napi(factory)]
  /// Create an agent from `config`.
  pub async fn create(config: AgentConfig) -> Result<Self> {
    let mut cfg: agent::AgentConfig = config.into();
    agent::agent::config::apply_model_defaults(&mut cfg);
    let js_hooks = HookRegistry::new();

    let mut llm_provider = agent::agent::agentic::LlmProvider::new();

    let (vendor_name, vendor) = agent::agent::agentic::build_vendor(&cfg);
    llm_provider.register_vendor(vendor_name, vendor);

    let agent = agent::Agent::new(cfg, Arc::new(llm_provider));

    Ok(Agent {
      inner: Arc::new(Mutex::new(agent)),
      bus_session: None,
      hooks: std::sync::Arc::new(std::sync::Mutex::new(js_hooks)),
      perf: std::sync::Arc::new(crate::perf::PerformanceMetrics::new()),
      stop_flag: std::sync::Arc::new(AtomicBool::new(false)),
      is_running: std::sync::Arc::new(AtomicBool::new(false)),
    })
  }

  #[napi]
  /// Create an agent from `config`, using `bus`.
  pub async fn create_with_bus(
    config: AgentConfig,
    _bus: &External<Arc<crate::Session>>,
  ) -> Result<Self> {
    let mut cfg: agent::AgentConfig = config.into();
    agent::agent::config::apply_model_defaults(&mut cfg);
    let js_hooks = HookRegistry::new();

    let mut llm_provider = agent::agent::agentic::LlmProvider::new();

    let (vendor_name, vendor) = agent::agent::agentic::build_vendor(&cfg);
    llm_provider.register_vendor(vendor_name, vendor);

    let session = _bus.as_ref().clone();
    let bus = bus::Bus::new(session);
    let agent = agent::Agent::new(cfg, Arc::new(llm_provider)).with_bus(bus);

    Ok(Agent {
      inner: Arc::new(Mutex::new(agent)),
      bus_session: Some(_bus.as_ref().clone()),
      hooks: std::sync::Arc::new(std::sync::Mutex::new(js_hooks)),
      perf: std::sync::Arc::new(crate::perf::PerformanceMetrics::new()),
      stop_flag: std::sync::Arc::new(AtomicBool::new(false)),
      is_running: std::sync::Arc::new(AtomicBool::new(false)),
    })
  }

  #[napi]
  /// Run a single task and return the answer.
  pub async fn run_simple(&self, task: Either<String, Vec<JsContent>>) -> Result<String> {
    if self.is_running.load(Ordering::SeqCst) {
      return Err(Error::new(
        napi::Status::GenericFailure,
        "Agent is already running".to_string(),
      ));
    }
    if self.stop_flag.load(Ordering::SeqCst) {
      self.stop_flag.store(false, Ordering::SeqCst);
      return Ok(String::new());
    }

    let task_content = match task {
      Either::A(s) => Content::Text(s),
      Either::B(parts) => vec_jscontent_to_content(parts),
    };
    self.is_running.store(true, Ordering::SeqCst);
    let result = {
      let guard = self.inner.lock().await;
      guard.run_simple(task_content).await
    };
    self.is_running.store(false, Ordering::SeqCst);

    result.map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  /// Run the ReAct loop for `task`.
  pub async fn react(&self, task: Either<String, Vec<JsContent>>) -> Result<String> {
    if self.is_running.load(Ordering::SeqCst) {
      return Err(Error::new(
        napi::Status::GenericFailure,
        "Agent is already running".to_string(),
      ));
    }
    if self.stop_flag.load(Ordering::SeqCst) {
      self.stop_flag.store(false, Ordering::SeqCst);
      return Ok(String::new());
    }

    let task_content = match task {
      Either::A(s) => Content::Text(s),
      Either::B(parts) => vec_jscontent_to_content(parts),
    };
    self.is_running.store(true, Ordering::SeqCst);
    let result = {
      let guard = self.inner.lock().await;
      guard.react(task_content).await
    };
    self.is_running.store(false, Ordering::SeqCst);

    result.map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  /// Return the effective agent configuration.
  pub fn config(&self) -> Result<serde_json::Value> {
    let guard = self.inner.blocking_lock();
    let cfg = guard.config();
    Ok(serde_json::json!({
        "name": cfg.name,
        "model": cfg.model,
        "base_url": cfg.base_url,
        "system_prompt": cfg.system_prompt,
        "temperature": cfg.temperature,
        "max_tokens": cfg.max_tokens,
        "timeout_secs": cfg.timeout_secs,
        "api_mode": cfg.api_mode,
        "reasoning_effort": cfg.reasoning_effort,
    }))
  }

  #[napi]
  /// List registered tool names.
  pub fn list_tools(&self) -> Result<Vec<String>> {
    let guard = self.inner.blocking_lock();
    if let Some(registry) = guard.registry() {
      // Tools added through the bindings are registered as async tools, so a
      // sync-only listing would hide every JS-registered tool.
      let mut names: Vec<String> = registry.iter().map(|(name, _)| name.clone()).collect();
      for name in registry.async_tool_names() {
        if !names.contains(&name) {
          names.push(name);
        }
      }
      Ok(names)
    } else {
      Ok(Vec::new())
    }
  }

  #[napi]
  /// List registered async tool names.
  pub fn list_async_tools(&self) -> Result<Vec<String>> {
    let guard = self.inner.blocking_lock();
    if let Some(registry) = guard.registry() {
      Ok(registry.async_tool_names())
    } else {
      Ok(Vec::new())
    }
  }

  #[napi]
  /// Register `hook` for `event`.
  pub fn register_hook(
    &self,
    event: HookEvent,
    callback: ThreadsafeFunction<
      HookContextData,
      napi::Unknown<'static>,
      HookContextData,
      napi::Status,
      true,
      true,
    >,
  ) -> Result<()> {
    let hook = crate::hooks::JSHook {
      callback: callback.into(),
    };
    let event = match event {
      HookEvent::BeforeToolCall => agent::agent::hooks::HookEvent::BeforeToolCall,
      HookEvent::AfterToolCall => agent::agent::hooks::HookEvent::AfterToolCall,
      HookEvent::BeforeLlmCall => agent::agent::hooks::HookEvent::BeforeLlmCall,
      HookEvent::AfterLlmCall => agent::agent::hooks::HookEvent::AfterLlmCall,
      HookEvent::OnMessage => agent::agent::hooks::HookEvent::OnMessage,
      HookEvent::OnComplete => agent::agent::hooks::HookEvent::OnComplete,
      HookEvent::OnError => agent::agent::hooks::HookEvent::OnError,
    };

    let guard = self.inner.blocking_lock();
    guard.hooks().register_blocking(event, Arc::new(hook));
    Ok(())
  }

  #[napi]
  /// Register `plugin` with the agent.
  pub fn register_plugin(
    &self,
    name: String,
    on_llm_request: Option<
      ThreadsafeFunction<JSAny, napi::Unknown<'static>, JSAny, napi::Status, true, true>,
    >,
    on_llm_response: Option<
      ThreadsafeFunction<JSAny, napi::Unknown<'static>, JSAny, napi::Status, true, true>,
    >,
    on_tool_call: Option<
      ThreadsafeFunction<JSAny, napi::Unknown<'static>, JSAny, napi::Status, true, true>,
    >,
    on_tool_result: Option<
      ThreadsafeFunction<JSAny, napi::Unknown<'static>, JSAny, napi::Status, true, true>,
    >,
  ) -> Result<()> {
    let js_plugin = crate::plugin::JSPlugin::new(
      name,
      on_llm_request,
      on_llm_response,
      on_tool_call,
      on_tool_result,
    );
    let plugin_arc: std::sync::Arc<dyn agent::agent::plugin::AgentPlugin> =
      std::sync::Arc::new(js_plugin);

    let mut guard = self.inner.blocking_lock();
    guard.add_plugin(plugin_arc);
    Ok(())
  }

  #[napi]
  /// Close the agent and release resources.
  pub fn close(&self) -> Result<()> {
    let mut guard = self.inner.blocking_lock();
    guard.clear_runtime_extensions();
    guard.stop();
    self.stop_flag.store(true, Ordering::SeqCst);
    self.is_running.store(false, Ordering::SeqCst);
    Ok(())
  }

  #[napi]
  /// Stop the running agent.
  pub fn stop(&self, options: Option<StopOptions>) -> Result<serde_json::Value> {
    let was_running = self.is_running.load(Ordering::SeqCst);
    self.stop_flag.store(true, Ordering::SeqCst);
    self.is_running.store(false, Ordering::SeqCst);

    if let Some(opts) = options {
      if opts.clear_session.unwrap_or(false) {
        let mut guard = self.inner.blocking_lock();
        guard.stop();
        guard.session_mut().clear();
      }
    }

    Ok(serde_json::json!({ "stopped": was_running }))
  }

  #[napi]
  /// Whether the agent is running.
  pub fn is_running(&self) -> Result<bool> {
    Ok(self.is_running.load(Ordering::SeqCst))
  }

  #[allow(clippy::too_many_arguments)] // positional JS arguments; a struct would change the JS API
  #[napi]
  /// Register a tool implemented in JavaScript.
  pub async fn add_tool(
    &self,
    name: String,
    description: String,
    _parameters: String,
    schema: String,
    callback: ThreadsafeFunction<JSAny, napi::Unknown<'static>, JSAny, napi::Status, true, true>,
    cancelable: bool,
    cancel_callback: Option<
      ThreadsafeFunction<String, napi::Unknown<'static>, String, napi::Status, true, true>,
    >,
  ) -> Result<String> {
    let tool = JSTool {
      name: name.clone(),
      description,
      schema: serde_json::from_str(&schema).unwrap_or(serde_json::Value::Null),
      cancelable,
      callback: callback.into(),
      cancel_callback: cancel_callback.map(Arc::new),
    };
    let mut guard = self.inner.lock().await;
    guard
      .try_add_async_tool(std::sync::Arc::new(tool))
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(name)
  }

  #[napi]
  /// Register a sandboxed bash tool.
  pub async fn add_bash_tool(&self, name: String, workspace_root: Option<String>) -> Result<()> {
    let tool = if let Some(root) = workspace_root {
      BashTool::new(&name).with_workspace(&root)
    } else {
      BashTool::new(&name)
    };
    let mut guard = self.inner.lock().await;
    guard
      .try_add_tool(std::sync::Arc::new(tool))
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(())
  }

  #[napi]
  /// Register skills discovered in `dir_path`.
  pub async fn register_skills_from_dir(&self, dir_path: String) -> Result<()> {
    let mut guard = self.inner.lock().await;
    guard
      .register_skills_from_dir(std::path::PathBuf::from(dir_path))
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  /// Add a stdio MCP server.
  pub async fn add_mcp_server(
    &self,
    namespace: String,
    command: String,
    args: Vec<String>,
  ) -> Result<()> {
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let client = agent::mcp::McpClient::spawn(&command, &args_ref)
      .await
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;

    client
      .initialize()
      .await
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;

    let client = std::sync::Arc::new(client);

    let mut guard = self.inner.lock().await;
    guard
      .register_mcp_tools_with_namespace(client, &namespace)
      .await
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  /// Add an HTTP MCP server under `namespace`.
  pub async fn add_mcp_server_http(&self, namespace: String, url: String) -> Result<()> {
    let client = agent::mcp::McpClient::connect_http(&url);
    let client = std::sync::Arc::new(client);

    client
      .initialize()
      .await
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;

    let mut guard = self.inner.lock().await;
    guard
      .register_mcp_tools_with_namespace(client, &namespace)
      .await
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  /// List tools across all MCP servers.
  pub async fn list_mcp_tools(&self) -> Result<Vec<serde_json::Value>> {
    let guard = self.inner.lock().await;
    Ok(
      guard
        .registry()
        .map(|registry| registry.mcp_tool_entries())
        .unwrap_or_default(),
    )
  }

  #[napi]
  /// List resources for an MCP namespace.
  pub async fn list_mcp_resources(&self, namespace: String) -> Result<Vec<serde_json::Value>> {
    let guard = self.inner.lock().await;
    Ok(
      guard
        .registry()
        .map(|registry| registry.mcp_resource_entries(&namespace))
        .unwrap_or_default(),
    )
  }

  #[napi]
  /// List prompts across all MCP servers.
  pub async fn list_mcp_prompts(&self) -> Result<Vec<serde_json::Value>> {
    let guard = self.inner.lock().await;
    Ok(
      guard
        .registry()
        .map(|registry| registry.mcp_prompt_entries())
        .unwrap_or_default(),
    )
  }

  #[napi]
  /// Build an RPC client for this agent.
  pub async fn rpc_client(
    &self,
    endpoint: String,
    _bus: &External<Arc<crate::Session>>,
  ) -> Result<AgentRpcClient> {
    let session = self.bus_session.clone().ok_or_else(|| {
      napi::Error::new(napi::Status::GenericFailure, "Agent not created with bus")
    })?;
    let agent = {
      let guard = self.inner.lock().await;
      guard.clone()
    };
    let client = agent.rpc_client(endpoint.clone(), session);
    Ok(AgentRpcClient {
      inner: std::sync::Arc::new(client),
    })
  }

  #[napi]
  /// Serve this agent as a callable server.
  pub async fn as_callable_server(
    &self,
    endpoint: String,
    _bus: &External<Arc<crate::Session>>,
  ) -> Result<AgentCallableServer> {
    let session = self.bus_session.clone().ok_or_else(|| {
      napi::Error::new(napi::Status::GenericFailure, "Agent not created with bus")
    })?;
    let agent = {
      let guard = self.inner.lock().await;
      guard.clone()
    };
    let mut server = agent.as_callable_server(endpoint.clone(), session);
    server
      .start()
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(AgentCallableServer {
      inner: std::sync::Arc::new(server),
    })
  }

  #[napi]
  /// Run `task` and stream tokens to `callback`.
  pub async fn stream(
    &self,
    task: Either<String, Vec<JsContent>>,
    callback: ThreadsafeFunction<
      serde_json::Value,
      napi::Unknown<'static>,
      serde_json::Value,
      napi::Status,
      true,
      true,
    >,
  ) -> Result<String> {
    if self.is_running.load(Ordering::SeqCst) {
      return Err(Error::new(
        napi::Status::GenericFailure,
        "Agent is already running".to_string(),
      ));
    }
    if self.stop_flag.load(Ordering::SeqCst) {
      self.stop_flag.store(false, Ordering::SeqCst);
      return Ok(serde_json::json!({ "status": "stopped" }).to_string());
    }

    let task_content = match task {
      Either::A(s) => Content::Text(s),
      Either::B(parts) => vec_jscontent_to_content(parts),
    };
    self.is_running.store(true, Ordering::SeqCst);

    let result = async {
      let guard = self.inner.lock().await;
      let start = std::time::Instant::now();

      let stream = guard.stream(task_content);
      use futures::StreamExt;
      futures::pin_mut!(stream);
      let mut had_error = false;
      let mut was_stopped = false;
      while let Some(token_result) = stream.next().await {
        if self.stop_flag.load(Ordering::SeqCst) {
          was_stopped = true;
          break;
        }
        match token_result {
          Ok(token) => {
            let json = match token {
              agent::StreamToken::Text(text) => {
                serde_json::json!({ "type": "Text", "text": text })
              }
              agent::StreamToken::ReasoningContent(text) => {
                serde_json::json!({ "type": "ReasoningContent", "text": text })
              }
              agent::StreamToken::ToolCall { name, args, id } => {
                serde_json::json!({
                    "type": "ToolCall",
                    "name": name,
                    "args": args,
                    "id": id
                })
              }
              agent::StreamToken::Usage(usage) => {
                serde_json::json!({
                    "type": "Usage",
                    "promptTokens": usage.prompt_tokens,
                    "completionTokens": usage.completion_tokens,
                    "totalTokens": usage.total_tokens,
                    "promptTokensDetails": usage.prompt_tokens_details.as_ref().map(|d| serde_json::json!({
                        "audioTokens": d.audio_tokens,
                        "cachedTokens": d.cached_tokens,
                    })),
                })
              }
              agent::StreamToken::Done => {
                serde_json::json!({ "type": "Done" })
              }
              agent::StreamToken::Stopped => {
                was_stopped = true;
                serde_json::json!({ "type": "Stopped" })
              }
            };
            callback.call(Ok(json), ThreadsafeFunctionCallMode::Blocking);
            if was_stopped {
              break;
            }
            }
            Err(e) => {
              had_error = true;
              let json = serde_json::json!({
                  "type": "Error",
                  "error": e.to_string()
              });
              callback.call(Ok(json), ThreadsafeFunctionCallMode::Blocking);
          }
        }
      }

      let elapsed = start.elapsed();
      if had_error {
        guard.record_llm_error();
      }
      let tokens = guard.last_token_usage().unwrap_or((0, 0));
      guard.record_stream_call(
        elapsed,
        elapsed,
        std::time::Duration::ZERO,
        tokens.0,
        tokens.1,
      );
      let tool_calls = guard.last_stream_tool_calls();
      if tool_calls > 0 {
        guard.record_tool_calls(tool_calls, std::time::Duration::ZERO);
      }

      let status = if was_stopped { "stopped" } else { "completed" };
      Ok(serde_json::json!({ "status": status }).to_string())
    }.await;

    self.is_running.store(false, Ordering::SeqCst);
    result
  }

  #[napi]
  /// Return the session history as JSON.
  pub fn get_session_json(&self) -> Result<String> {
    let guard = self.inner.blocking_lock();
    let session = guard.session();
    let value = serde_json::to_value(&*session)
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;
    serde_json::to_string_pretty(&value)
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  /// Export the session as JSON.
  pub fn export_session(&self) -> Result<String> {
    self.get_session_json()
  }

  #[napi]
  /// Restore the session from JSON.
  pub fn restore_session_json(&self, json: String) -> Result<()> {
    let mut guard = self.inner.blocking_lock();
    let result = guard.session_mut().restore_from_json(&json);
    match result {
      Ok(()) => Ok(()),
      Err(e) => Err(Error::new(napi::Status::GenericFailure, e.to_string())),
    }
  }

  #[napi]
  /// Save the session to `path`.
  pub fn save_session(&self, path: String) -> Result<()> {
    let json = self.get_session_json()?;
    std::fs::write(&path, json).map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  /// Restore the session from `path`.
  pub fn restore_session_from_file(&self, path: String) -> Result<()> {
    let json = std::fs::read_to_string(&path)
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;
    self.restore_session_json(json)
  }

  #[napi]
  /// Clear the session history.
  pub fn clear_session(&self) -> Result<()> {
    let mut guard = self.inner.blocking_lock();
    guard.session_mut().clear();
    Ok(())
  }

  #[napi]
  /// Compact the session, keeping recent turns.
  pub fn compact_session(&self, keep_recent: u32, max_summary_chars: u32) -> Result<()> {
    let mut guard = self.inner.blocking_lock();
    guard
      .session_mut()
      .compact(keep_recent as usize, max_summary_chars as usize);
    Ok(())
  }

  /// Persist the agent's conversation to a file path.
  #[napi]
  pub fn save_message_log(&self, path: String) -> Result<()> {
    self.save_session(path)
  }

  /// Load a previously saved conversation from a file path.
  #[napi]
  pub fn restore_message_log(&self, path: String) -> Result<()> {
    self.restore_session_from_file(path)
  }

  /// The conversation messages, serialized as JSON.
  #[napi]
  pub fn get_messages(&self) -> Result<serde_json::Value> {
    let guard = self.inner.blocking_lock();
    let messages = guard.session().messages().to_vec();
    serde_json::to_value(messages)
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  /// Append a role/content message to the conversation.
  #[napi]
  pub fn add_message(&self, message: serde_json::Value) -> Result<()> {
    let role = message
      .get("role")
      .and_then(|v| v.as_str())
      .ok_or_else(|| Error::new(napi::Status::InvalidArg, "message.role is required"))?;
    let content = message
      .get("content")
      .and_then(|v| v.as_str())
      .unwrap_or_default();
    let msg = match role {
      "system" => react::llm::LlmMessage::system(content),
      "user" => react::llm::LlmMessage::user_text(content),
      "assistant" => react::llm::LlmMessage::Assistant {
        content: content.to_string(),
      },
      other => {
        return Err(Error::new(
          napi::Status::InvalidArg,
          format!("unsupported message role: {other}"),
        ))
      }
    };
    let mut guard = self.inner.blocking_lock();
    guard.add_message(msg);
    Ok(())
  }

  #[napi]
  /// Return a snapshot of performance metrics.
  pub fn get_perf_metrics(&self) -> crate::perf::PerfSnapshot {
    let guard = self.inner.blocking_lock();
    let cm = guard.metrics();
    crate::perf::PerfSnapshot {
      llm_call_count: cm.llm_call_count as i64,
      total_wall_time_us: cm.total_wall_time.as_micros() as i64,
      avg_wall_time_us: if cm.llm_call_count > 0 {
        cm.total_wall_time.as_micros() as i64 / cm.llm_call_count as i64
      } else {
        0
      },
      min_wall_time_us: 0,
      max_wall_time_us: 0,
      total_engine_time_us: cm.total_engine_time.as_micros() as i64,
      total_resilience_time_us: cm.total_resilience_time.as_micros() as i64,
      rate_limit_waits: cm.rate_limit_waits as i64,
      total_rate_limit_wait_us: cm.total_rate_limit_wait.as_micros() as i64,
      circuit_trips: cm.circuit_trips as i64,
      llm_errors: cm.llm_errors as i64,
      tool_invocation_count: cm.tool_invocation_count as i64,
      total_tool_time_us: cm.total_tool_time.as_micros() as i64,
      total_input_tokens: cm.total_input_tokens as i64,
      total_output_tokens: cm.total_output_tokens as i64,
    }
  }

  #[napi]
  /// Reset all performance metrics.
  pub fn reset_perf_metrics(&self) {
    let guard = self.inner.blocking_lock();
    guard.reset_metrics();
    self.perf.reset();
  }
}

#[napi]
/// Client for calling a remote agent.
pub struct AgentRpcClient {
  inner: std::sync::Arc<agent::bus::AgentRpcClient>,
}

#[napi]
impl AgentRpcClient {
  /// The RPC endpoint.
  #[napi(getter)]
  pub fn endpoint(&self) -> String {
    self.inner.endpoint().to_string()
  }

  #[napi]
  /// List the remote tools.
  pub async fn list(&self) -> Result<serde_json::Value> {
    let tools = self
      .inner
      .list()
      .await
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(tools)
  }

  #[napi]
  /// Call a remote tool.
  pub async fn call(&self, tool_name: String, args_json: String) -> Result<serde_json::Value> {
    let args: serde_json::Value = serde_json::from_str(&args_json)
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;
    let result = self
      .inner
      .call(&tool_name, args)
      .await
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(result)
  }
}

#[napi]
/// Serves calls for a JavaScript agent.
pub struct AgentCallableServer {
  inner: std::sync::Arc<agent::bus::AgentCallableServer>,
}

#[napi]
impl AgentCallableServer {
  /// The server endpoint.
  #[napi(getter)]
  pub fn endpoint(&self) -> String {
    self.inner.endpoint().to_string()
  }

  #[napi]
  /// Whether the server has started.
  pub fn is_started(&self) -> bool {
    true
  }
}

impl Drop for Agent {
  fn drop(&mut self) {
    if let Ok(mut guard) = self.inner.try_lock() {
      guard.clear_runtime_extensions();
      guard.stop();
    }
    self.stop_flag.store(true, Ordering::SeqCst);
    self.is_running.store(false, Ordering::SeqCst);
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use agent::tools::registry::ToolRegistry;

  struct MockAsyncTool {
    name: String,
    description: String,
  }

  #[async_trait]
  impl AsyncTool for MockAsyncTool {
    fn name(&self) -> &str {
      &self.name
    }

    fn description(&self) -> String {
      self.description.clone()
    }

    async fn run(
      &self,
      _args: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, react::ToolError> {
      Ok(serde_json::json!({ "result": "async_executed" }))
    }
  }

  #[tokio::test]
  async fn test_async_tool_registration() {
    let mut registry = ToolRegistry::new();
    let tool = Arc::new(MockAsyncTool {
      name: "test_async".to_string(),
      description: "Test async tool".to_string(),
    });
    registry.register_async(tool).unwrap();

    assert!(registry
      .async_tool_names()
      .contains(&"test_async".to_string()));
    assert_eq!(registry.list().len(), 1);
  }

  #[tokio::test]
  async fn test_async_tool_execution() {
    let mut registry = ToolRegistry::new();
    let tool = Arc::new(MockAsyncTool {
      name: "test_exec".to_string(),
      description: "Test execution".to_string(),
    });
    registry.register_async(tool).unwrap();

    let result = registry
      .execute("test_exec", &serde_json::json!({}))
      .unwrap();
    assert_eq!(result["result"], "async_executed");
  }
}
