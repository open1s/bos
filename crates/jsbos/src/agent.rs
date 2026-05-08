#![allow(clippy::unnecessary_cast)]

use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::hooks::{HookContextData, HookEvent, HookRegistry};
use crate::jsany::JSAny;
use crate::plugin::PluginRegistry;
use agent::BashTool;
use react::llm::vendor::{NvidiaVendor, OpenAiClient, OpenRouterVendor};

struct JSTool {
  name: String,
  description: String,
  schema: serde_json::Value,
  callback: Arc<ThreadsafeFunction<JSAny, napi::Unknown<'static>>>,
}

#[async_trait]
impl agent::Tool for JSTool {
  fn name(&self) -> &str {
    &self.name
  }

  fn description(&self) -> String {
    self.description.clone()
  }

  fn json_schema(&self) -> serde_json::Value {
    self.schema.clone()
  }

  fn run(
    &self,
    args: &serde_json::Value,
  ) -> std::result::Result<serde_json::Value, react::ToolError> {
    let args_json = args.clone();
    let callback = self.callback.clone();

    let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<serde_json::Value, String>>();
    let tx_clone = tx.clone();

    callback.call_with_return_value(
      Ok(JSAny(args_json)),
      ThreadsafeFunctionCallMode::NonBlocking,
      move |result: std::result::Result<napi::Unknown<'_>, napi::Error>,
            _env|
            -> napi::Result<()> {
        match result {
          Ok(val) => {
            let utf8 = val
              .coerce_to_string()?
              .into_utf8()?;
            let string_val = utf8.as_str()?;
            let json_val: serde_json::Value = serde_json::from_str(string_val)
              .unwrap_or_else(|_| serde_json::json!(string_val));
            let _ = tx_clone.send(Ok(json_val));
          }
          Err(e) => {
            let _ = tx_clone.send(Err(e.to_string()));
          }
        }
        Ok(())
      },
    );

    match rx.recv() {
      Ok(Ok(result)) => std::result::Result::Ok(result),
      Ok(Err(e)) => std::result::Result::Err(react::ToolError::Failed(e.to_string())),
      Err(_) => std::result::Result::Err(react::ToolError::Failed(
        "handler channel closed".to_string(),
      )),
    }
  }
}

#[napi(object)]
pub struct AgentConfig {
  pub name: String,
  pub model: String,
  pub base_url: String,
  pub api_key: String,
  pub system_prompt: String,
  pub temperature: f64,
  pub max_tokens: Option<i32>,
  pub timeout_secs: i64,
  pub max_steps: Option<i64>,
  pub circuit_breaker_max_failures: Option<i32>,
  pub circuit_breaker_cooldown_secs: Option<i64>,
  pub rate_limit_capacity: Option<i32>,
  pub rate_limit_window_secs: Option<i64>,
  pub rate_limit_max_retries: Option<i32>,
  pub rate_limit_retry_backoff_secs: Option<i64>,
  pub rate_limit_auto_wait: Option<bool>,
  pub context_compaction_threshold_tokens: Option<i32>,
  pub context_compaction_trigger_ratio: Option<f64>,
  pub context_compaction_keep_recent_messages: Option<i32>,
  pub context_compaction_max_summary_chars: Option<i32>,
  pub context_compaction_summary_max_tokens: Option<i32>,
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
      circuit_breaker_max_failures: None,
      circuit_breaker_cooldown_secs: None,
      rate_limit_capacity: None,
      rate_limit_window_secs: None,
      rate_limit_max_retries: None,
      rate_limit_retry_backoff_secs: None,
      rate_limit_auto_wait: None,
      context_compaction_threshold_tokens: None,
      context_compaction_trigger_ratio: None,
      context_compaction_keep_recent_messages: None,
      context_compaction_max_summary_chars: None,
      context_compaction_summary_max_tokens: None,
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
      circuit_breaker,
      rate_limit,
      context_compaction_threshold_tokens: value.context_compaction_threshold_tokens.unwrap_or(0)
        as usize,
      context_compaction_trigger_ratio: value.context_compaction_trigger_ratio.unwrap_or(0.0)
        as f32,
      context_compaction_keep_recent_messages: value
        .context_compaction_keep_recent_messages
        .unwrap_or(0) as usize,
      context_compaction_max_summary_chars: value.context_compaction_max_summary_chars.unwrap_or(0)
        as usize,
      context_compaction_summary_max_tokens: value
        .context_compaction_summary_max_tokens
        .unwrap_or(0) as u32,
    }
  }
}

#[napi]
pub struct Agent {
  inner: Arc<Mutex<agent::Agent>>,
  bus_session: Option<Arc<crate::Session>>,
  #[allow(dead_code)]
  hooks: std::sync::Arc<std::sync::Mutex<HookRegistry>>,
  plugins: std::sync::Arc<std::sync::Mutex<PluginRegistry>>,
  perf: std::sync::Arc<crate::perf::PerformanceMetrics>,
}

#[napi]
  impl Agent {
  #[napi(factory)]
  pub async fn create(config: AgentConfig) -> Result<Self> {
    let cfg: agent::AgentConfig = config.into();
    let js_hooks = HookRegistry::new();
    let js_plugins = PluginRegistry::new();

    let mut llm_provider = agent::agent::agentic::LlmProvider::new();

    let (vendor_name, model_name) = if let Some(pos) = cfg.model.find('/') {
      (cfg.model[..pos].to_string(), cfg.model[pos + 1..].to_string())
    } else {
      ("openai".to_string(), cfg.model.clone())
    };

    let vendor: Box<dyn react::llm::LlmClient<_, _>> = match vendor_name.as_str() {
      "nvidia" => Box::new(NvidiaVendor::new(
        cfg.base_url.clone(),
        model_name,
        cfg.api_key.clone(),
      )),
      "openrouter" => Box::new(OpenRouterVendor::new(
        cfg.base_url.clone(),
        model_name,
        cfg.api_key.clone(),
      )),
      _ => Box::new(OpenAiClient::new(
        cfg.base_url.clone(),
        model_name,
        cfg.api_key.clone(),
      )),
    };
    llm_provider.register_vendor(vendor_name, vendor);

    let agent = agent::Agent::new(cfg, Arc::new(llm_provider));

    Ok(Agent {
      inner: Arc::new(Mutex::new(agent)),
      bus_session: None,
      hooks: std::sync::Arc::new(std::sync::Mutex::new(js_hooks)),
      plugins: std::sync::Arc::new(std::sync::Mutex::new(js_plugins)),
      perf: std::sync::Arc::new(crate::perf::PerformanceMetrics::new()),
    })
  }

  #[napi]
  pub async fn create_with_bus(
    config: AgentConfig,
    _bus: &External<Arc<crate::Session>>,
  ) -> Result<Self> {
    let cfg: agent::AgentConfig = config.into();
    let js_hooks = HookRegistry::new();
    let js_plugins = PluginRegistry::new();

    let mut llm_provider = agent::agent::agentic::LlmProvider::new();

    let (vendor_name, model_name) = if let Some(pos) = cfg.model.find('/') {
      (cfg.model[..pos].to_string(), cfg.model[pos + 1..].to_string())
    } else {
      ("openai".to_string(), cfg.model.clone())
    };

    let vendor: Box<dyn react::llm::LlmClient<_, _>> = match vendor_name.as_str() {
      "nvidia" => Box::new(NvidiaVendor::new(
        cfg.base_url.clone(),
        model_name,
        cfg.api_key.clone(),
      )),
      "openrouter" => Box::new(OpenRouterVendor::new(
        cfg.base_url.clone(),
        model_name,
        cfg.api_key.clone(),
      )),
      _ => Box::new(OpenAiClient::new(
        cfg.base_url.clone(),
        model_name,
        cfg.api_key.clone(),
      )),
    };
    llm_provider.register_vendor(vendor_name, vendor);

    let agent = agent::Agent::new(cfg, Arc::new(llm_provider));

    Ok(Agent {
      inner: Arc::new(Mutex::new(agent)),
      bus_session: None,
      hooks: std::sync::Arc::new(std::sync::Mutex::new(js_hooks)),
      plugins: std::sync::Arc::new(std::sync::Mutex::new(js_plugins)),
      perf: std::sync::Arc::new(crate::perf::PerformanceMetrics::new()),
    })
  }

  #[napi]
  pub async fn run_simple(&self, task: String) -> Result<String> {
    let guard = self.inner.lock().await;
    guard.run_simple(&task).await
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  pub async fn react(&self, task: String) -> Result<String> {
    let guard = self.inner.lock().await;
    guard.react(&task).await
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
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
    }))
  }

  #[napi]
  pub fn list_tools(&self) -> Result<Vec<String>> {
    let guard = self.inner.blocking_lock();
    if let Some(registry) = guard.registry() {
      Ok(registry.iter().map(|(name, _)| name.clone()).collect())
    } else {
      Ok(Vec::new())
    }
  }

  #[napi]
  pub fn register_hook(
    &self,
    event: HookEvent,
    callback: ThreadsafeFunction<HookContextData>,
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
  pub fn register_plugin(
    &self,
    name: String,
    on_llm_request: Option<ThreadsafeFunction<JSAny>>,
    on_llm_response: Option<ThreadsafeFunction<JSAny>>,
    on_tool_call: Option<ThreadsafeFunction<JSAny>>,
    on_tool_result: Option<ThreadsafeFunction<JSAny>>,
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

    let plugins_guard = self.plugins.lock().unwrap();
    let inner = plugins_guard.inner().clone();
    drop(plugins_guard);
    inner.register_blocking(plugin_arc);
    Ok(())
  }

  #[napi]
  pub fn close(&self) -> Result<()> {
    let mut guard = self.inner.blocking_lock();
    guard.clear_runtime_extensions();

    Ok(())
  }

  #[napi]
  pub async fn add_tool(
    &self,
    name: String,
    description: String,
    _parameters: String,
    schema: String,
    callback: ThreadsafeFunction<JSAny>,
  ) -> Result<String> {
    let tool = JSTool {
      name: name.clone(),
      description,
      schema: serde_json::from_str(&schema).unwrap_or(serde_json::Value::Null),
      callback: callback.into(),
    };
    let mut guard = self.inner.lock().await;
    guard
      .try_add_tool(std::sync::Arc::new(tool))
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(name)
  }

  #[napi]
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
  pub async fn register_skills_from_dir(&self, dir_path: String) -> Result<()> {
    let mut guard = self.inner.lock().await;
    guard
      .register_skills_from_dir(std::path::PathBuf::from(dir_path))
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
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
  pub async fn list_mcp_tools(&self) -> Result<Vec<serde_json::Value>> {
    let guard = self.inner.lock().await;
    if let Some(registry) = guard.registry() {
      let tools: Vec<serde_json::Value> = registry
        .iter()
        .filter(|(name, _)| name.contains('/'))
        .map(|(name, tool)| {
          serde_json::json!({
              "name": name,
              "description": tool.description(),
          })
        })
        .collect();
      Ok(tools)
    } else {
      Ok(Vec::new())
    }
  }

  #[napi]
  pub async fn list_mcp_resources(&self, namespace: String) -> Result<Vec<serde_json::Value>> {
    let guard = self.inner.lock().await;
    if let Some(registry) = guard.registry() {
      let resources: Vec<serde_json::Value> = registry
        .iter()
        .filter(|(name, _)| name.starts_with(&format!("{}/", namespace)))
        .map(|(name, tool)| {
          serde_json::json!({
              "name": name,
              "description": tool.description(),
          })
        })
        .collect();
      Ok(resources)
    } else {
      Ok(Vec::new())
    }
  }

  #[napi]
  pub async fn list_mcp_prompts(&self) -> Result<Vec<serde_json::Value>> {
    let guard = self.inner.lock().await;
    if let Some(registry) = guard.registry() {
      let prompts: Vec<serde_json::Value> = registry
        .iter()
        .filter(|(name, _)| name.contains('/'))
        .map(|(name, tool)| {
          serde_json::json!({
              "name": name,
              "description": tool.description(),
          })
        })
        .collect();
      Ok(prompts)
    } else {
      Ok(Vec::new())
    }
  }

  #[napi]
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
  pub async fn stream(
    &self,
    task: String,
    callback: ThreadsafeFunction<serde_json::Value>,
  ) -> Result<()> {
    let guard = self.inner.lock().await;

    let stream = guard.stream(&task);
    use futures::StreamExt;
    futures::pin_mut!(stream);
    while let Some(token_result) = stream.next().await {
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
            agent::StreamToken::Done => {
              serde_json::json!({ "type": "Done" })
            }
          };
          callback.call(Ok(json), ThreadsafeFunctionCallMode::NonBlocking);
        }
        Err(e) => {
          let json = serde_json::json!({
              "type": "Error",
              "error": e.to_string()
          });
          callback.call(Ok(json), ThreadsafeFunctionCallMode::NonBlocking);
        }
      }
    }
    Ok(())
  }

  #[napi]
  pub fn get_session_json(&self) -> Result<String> {
    let guard = self.inner.blocking_lock();
    let session = guard.session();
    let value = serde_json::to_value(&*session)
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;
    serde_json::to_string_pretty(&value)
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  pub fn export_session(&self) -> Result<String> {
    self.get_session_json()
  }

  #[napi]
  pub fn restore_session_json(&self, json: String) -> Result<()> {
    let mut guard = self.inner.blocking_lock();
    let result = guard.session_mut().restore_from_json(&json);
    match result {
      Ok(()) => Ok(()),
      Err(e) => Err(Error::new(napi::Status::GenericFailure, e.to_string()))
    }
  }

  #[napi]
  pub fn save_session(&self, path: String) -> Result<()> {
    let json = self.get_session_json()?;
    std::fs::write(&path, json)
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  pub fn restore_session_from_file(&self, path: String) -> Result<()> {
    let json = std::fs::read_to_string(&path)
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;
    self.restore_session_json(json)
  }

  #[napi]
  pub fn clear_session(&self) -> Result<()> {
    let mut guard = self.inner.blocking_lock();
    guard.session_mut().clear();
    Ok(())
  }

  #[napi]
  pub fn compact_session(&self, keep_recent: u32, max_summary_chars: u32) -> Result<()> {
    let mut guard = self.inner.blocking_lock();
    guard.session_mut().compact(keep_recent as usize, max_summary_chars as usize);
    Ok(())
  }

  #[napi]
  pub fn get_perf_metrics(&self) -> crate::perf::PerfSnapshot {
    let guard = self.inner.blocking_lock();
    let cm = guard.metrics();
    crate::perf::PerfSnapshot {
      call_count: cm.call_count as i64,
      total_wall_time_us: cm.total_wall_time.as_micros() as i64,
      avg_wall_time_us: if cm.call_count > 0 { cm.total_wall_time.as_micros() as i64 / cm.call_count as i64 } else { 0 },
      min_wall_time_us: 0,
      max_wall_time_us: 0,
      total_engine_time_us: cm.total_engine_time.as_micros() as i64,
      total_resilience_time_us: cm.total_resilience_time.as_micros() as i64,
      rate_limit_waits: cm.rate_limit_waits as i64,
      total_rate_limit_wait_us: cm.total_rate_limit_wait.as_micros() as i64,
      circuit_trips: cm.circuit_trips as i64,
      llm_errors: cm.llm_errors as i64,
      tool_call_count: cm.tool_call_count as i64,
      total_tool_time_us: cm.total_tool_time.as_micros() as i64,
      total_input_tokens: cm.total_input_tokens as i64,
      total_output_tokens: cm.total_output_tokens as i64,
    }
  }

  #[napi]
  pub fn reset_perf_metrics(&self) {
    let guard = self.inner.blocking_lock();
    guard.reset_metrics();
    self.perf.reset();
  }
}

#[napi]
pub struct AgentRpcClient {
  inner: std::sync::Arc<agent::bus::AgentRpcClient>,
}

#[napi]
impl AgentRpcClient {
  #[napi(getter)]
  pub fn endpoint(&self) -> String {
    self.inner.endpoint().to_string()
  }

  #[napi]
  pub async fn list(&self) -> Result<serde_json::Value> {
    let tools = self
      .inner
      .list()
      .await
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(tools)
  }

  #[napi]
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
pub struct AgentCallableServer {
  inner: std::sync::Arc<agent::bus::AgentCallableServer>,
}

#[napi]
impl AgentCallableServer {
  #[napi(getter)]
  pub fn endpoint(&self) -> String {
    self.inner.endpoint().to_string()
  }

  #[napi]
  pub fn is_started(&self) -> bool {
    true
  }
}
