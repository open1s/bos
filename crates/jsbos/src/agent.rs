#![allow(clippy::unnecessary_cast)]

use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::jsany::JSAny;

struct JSTool {
  name: String,
  description: agent::ToolDescription,
  schema: serde_json::Value,
  callback: Arc<ThreadsafeFunction<JSAny, napi::Unknown<'static>>>,
}

#[async_trait]
impl agent::Tool for JSTool {
  fn name(&self) -> &str {
    &self.name
  }

  fn description(&self) -> agent::ToolDescription {
    self.description.clone()
  }

  fn json_schema(&self) -> serde_json::Value {
    self.schema.clone()
  }

  async fn execute(
    &self,
    args: &serde_json::Value,
  ) -> std::result::Result<serde_json::Value, agent::ToolError> {
    let args_json = args.clone();
    let callback = self.callback.clone();

    let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<serde_json::Value, String>>();
    let tx_clone = tx.clone();

    callback.call_with_return_value(
      Ok(JSAny(args_json)),
      ThreadsafeFunctionCallMode::Blocking,
      move |result: std::result::Result<napi::Unknown<'_>, napi::Error>,
            _env|
            -> napi::Result<()> {
        match result {
          Ok(val) => {
            let json_val: serde_json::Value = val
              .coerce_to_string()?
              .into_utf8()?
              .as_str()?
              .parse()
              .unwrap_or(serde_json::Value::Null);
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
      Ok(Err(e)) => std::result::Result::Err(agent::ToolError::ExecutionFailed(e.to_string())),
      Err(_) => std::result::Result::Err(agent::ToolError::ExecutionFailed(
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
    Self {
      name: value.name,
      model: value.model,
      base_url: value.base_url,
      api_key: value.api_key,
      system_prompt: value.system_prompt,
      temperature: value.temperature as f32,
      max_tokens: value.max_tokens.map(|v| v as u32),
      timeout_secs: value.timeout_secs as u64,
      max_steps: value.max_steps.unwrap_or(10) as usize,
      rate_limit: None,
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
}

#[napi]
impl Agent {
  #[napi(factory)]
  pub async fn create(config: AgentConfig) -> Result<Self> {
    let cfg: agent::AgentConfig = config.into();
    let agent = agent::Agent::builder()
      .name(cfg.name)
      .model(cfg.model)
      .base_url(cfg.base_url)
      .api_key(cfg.api_key)
      .system_prompt(cfg.system_prompt)
      .temperature(cfg.temperature)
      .max_tokens(cfg.max_tokens.unwrap_or(4096) as u32)
      .timeout(cfg.timeout_secs as u64)
      .build()
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;

    Ok(Agent {
      inner: Arc::new(Mutex::new(agent)),
      bus_session: None,
    })
  }

  #[napi]
  pub async fn create_with_bus(
    config: AgentConfig,
    _bus: &External<Arc<crate::Session>>,
  ) -> Result<Self> {
    let cfg: agent::AgentConfig = config.into();
    let agent = agent::Agent::builder()
      .name(cfg.name)
      .model(cfg.model)
      .base_url(cfg.base_url)
      .api_key(cfg.api_key)
      .system_prompt(cfg.system_prompt)
      .temperature(cfg.temperature)
      .max_tokens(cfg.max_tokens.unwrap_or(4096) as u32)
      .timeout(cfg.timeout_secs as u64)
      .build()
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;

    Ok(Agent {
      inner: Arc::new(Mutex::new(agent)),
      bus_session: None,
    })
  }

  #[napi]
  pub async fn run_simple(&self, task: String) -> Result<String> {
    let agent = {
      let guard = self.inner.lock().await;
      guard.clone()
    };
    agent
      .run_simple(&task)
      .await
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  pub async fn react(&self, task: String) -> Result<String> {
    let agent = {
      let guard = self.inner.lock().await;
      guard.clone()
    };
    agent
      .react(&task)
      .await
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
  pub async fn add_tool(
    &self,
    name: String,
    description: String,
    parameters: String,
    schema: String,
    callback: ThreadsafeFunction<JSAny>,
  ) -> Result<String> {
    let tool = JSTool {
      name: name.clone(),
      description: agent::ToolDescription {
        short: description,
        parameters,
      },
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
              "description": tool.description().short,
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
              "description": tool.description().short,
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
              "description": tool.description().short,
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
  pub async fn add_message(&self, message: serde_json::Value) -> Result<()> {
    let llm_message = if let Some(role) = message.get("role").and_then(|v| v.as_str()) {
      match role {
        "system" => {
          let content = message.get("content").and_then(|v| v.as_str()).unwrap_or("");
          agent::LlmMessage::System { content: content.to_string() }
        }
        "user" => {
          let content = message.get("content").and_then(|v| v.as_str()).unwrap_or("");
          agent::LlmMessage::User { content: content.to_string() }
        }
        "assistant" => {
          let content = message.get("content").and_then(|v| v.as_str()).unwrap_or("");
          agent::LlmMessage::Assistant { content: content.to_string() }
        }
        "assistant_tool_call" => {
          let tool_call_id = message.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
          let name = message.get("name").and_then(|v| v.as_str()).unwrap_or("");
          let args = message.get("args").cloned().unwrap_or(serde_json::Value::Null);
          agent::LlmMessage::AssistantToolCall {
            tool_call_id: tool_call_id.to_string(),
            name: name.to_string(),
            args,
          }
        }
        "tool_result" => {
          let tool_call_id = message.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
          let content = message.get("content").and_then(|v| v.as_str()).unwrap_or("");
          agent::LlmMessage::ToolResult {
            tool_call_id: tool_call_id.to_string(),
            content: content.to_string(),
          }
        }
        _ => {
          return Err(Error::new(
            napi::Status::GenericFailure,
            format!("Invalid role: {}", role),
          ))
        }
      }
    } else {
      return Err(Error::new(
        napi::Status::GenericFailure,
        "Message must have a 'role' field",
      ))
    };
    let mut guard = self.inner.lock().await;
    guard.add_message(llm_message);
    Ok(())
  }

  #[napi]
  pub fn get_messages(&self) -> Result<Vec<serde_json::Value>> {
    let guard = self.inner.blocking_lock();
    let messages = guard.get_messages();
    let json_messages: Vec<serde_json::Value> = messages
      .iter()
      .map(|msg| {
        match msg {
          agent::LlmMessage::System { content } => {
            serde_json::json!({
              "role": "system",
              "content": content
            })
          }
          agent::LlmMessage::User { content } => {
            serde_json::json!({
              "role": "user",
              "content": content
            })
          }
          agent::LlmMessage::Assistant { content } => {
            serde_json::json!({
              "role": "assistant",
              "content": content
            })
          }
          agent::LlmMessage::AssistantToolCall { tool_call_id, name, args } => {
            serde_json::json!({
              "role": "assistant_tool_call",
              "tool_call_id": tool_call_id,
              "name": name,
              "args": args
            })
          }
          agent::LlmMessage::ToolResult { tool_call_id, content } => {
            serde_json::json!({
              "role": "tool_result",
              "tool_call_id": tool_call_id,
              "content": content
            })
          }
        }
      })
      .collect();
    Ok(json_messages)
  }

  #[napi]
  pub fn save_message_log(&self, path: String) -> Result<()> {
    let guard = self.inner.blocking_lock();
    guard
      .save_message_log(&path)
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  pub fn restore_message_log(&self, path: String) -> Result<()> {
    let mut guard = self.inner.blocking_lock();
    guard
      .restore_message_log(&path)
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))
  }
}

#[napi]
pub struct AgentRpcClient {
  inner: std::sync::Arc<agent::bus_rpc::AgentRpcClient>,
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

  #[napi]
  pub async fn llm_run(&self, task: String) -> Result<serde_json::Value> {
    let result = self
      .inner
      .llm_run(&task)
      .await
      .map_err(|e| Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(result)
  }
}

#[napi]
pub struct AgentCallableServer {
  inner: std::sync::Arc<agent::bus_rpc::AgentCallableServer>,
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
