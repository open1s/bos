#![allow(clippy::unnecessary_cast)]

use agent::agent::plugin::{
  AgentPlugin, LlmRequestWrapper, LlmResponseWrapper, PluginRegistry as InnerPluginRegistry,
  ToolCallWrapper, ToolResultWrapper,
};
use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice};
use std::collections::HashMap;
use std::sync::Arc;

use crate::jsany::JSAny;

#[napi]
pub enum PluginStage {
  PreRequest,
  PostResponse,
  PreExecute,
  PostExecute,
}

#[napi(object)]
pub struct PluginLlmRequest {
  pub model: String,
  pub temperature: Option<f64>,
  pub max_tokens: Option<u32>,
  pub top_p: Option<f64>,
  pub top_k: Option<u32>,
  pub metadata: HashMap<String, String>,
}

impl From<PluginLlmRequest> for LlmRequestWrapper {
  fn from(req: PluginLlmRequest) -> Self {
    LlmRequestWrapper {
      model: req.model,
      temperature: req.temperature.map(|t| t as f32),
      max_tokens: req.max_tokens,
      top_p: req.top_p.map(|p| p as f32),
      top_k: req.top_k,
      metadata: req.metadata,
    }
  }
}

impl From<LlmRequestWrapper> for PluginLlmRequest {
  fn from(wrapper: LlmRequestWrapper) -> Self {
    PluginLlmRequest {
      model: wrapper.model,
      temperature: wrapper.temperature.map(|t| t as f64),
      max_tokens: wrapper.max_tokens,
      top_p: wrapper.top_p.map(|p| p as f64),
      top_k: wrapper.top_k,
      metadata: wrapper.metadata,
    }
  }
}

#[napi]
pub enum PluginLlmResponse {
  OpenAI {
    id: String,
    model: String,
    content: Option<String>,
  },
}

#[napi]
pub struct PluginToolCallInfo {
  pub id: String,
  pub name: String,
  pub arguments: String,
}

impl From<PluginLlmResponse> for LlmResponseWrapper {
  fn from(resp: PluginLlmResponse) -> Self {
    match resp {
      PluginLlmResponse::OpenAI { id, model, content } => {
        let choices = vec![Choice {
          index: 0,
          message: ChatMessage {
            role: "assistant".to_string(),
            content: content.clone(),
            tool_calls: None,
            function_call: None,
            reasoning_content: None,
            extra: serde_json::Value::Object(serde_json::Map::new()),
          },
          finish_reason: Some("stop".to_string()),
          stop_reason: None,
          logprobs: None,
        }];
        LlmResponseWrapper::OpenAI(ChatCompletionResponse {
          id,
          object: "chat.completion".to_string(),
          created: 0,
          model,
          choices,
          usage: None,
          system_fingerprint: None,
          nvext: None,
        })
      }
    }
  }
}

impl From<LlmResponseWrapper> for PluginLlmResponse {
  fn from(wrapper: LlmResponseWrapper) -> Self {
    match wrapper {
      LlmResponseWrapper::OpenAI(rsp) => {
        let choice = rsp.choices.first();
        PluginLlmResponse::OpenAI {
          id: rsp.id,
          model: rsp.model,
          content: choice.and_then(|c| c.message.content.clone()),
        }
      }
    }
  }
}

#[napi(object)]
pub struct PluginToolCall {
  pub name: String,
  pub args: String,
  pub id: Option<String>,
  pub metadata: HashMap<String, String>,
}

impl From<PluginToolCall> for ToolCallWrapper {
  fn from(call: PluginToolCall) -> Self {
    let args: serde_json::Value = serde_json::from_str(&call.args).unwrap_or_default();
    ToolCallWrapper {
      name: call.name,
      args,
      id: call.id,
      metadata: call.metadata,
    }
  }
}

impl From<ToolCallWrapper> for PluginToolCall {
  fn from(wrapper: ToolCallWrapper) -> Self {
    PluginToolCall {
      name: wrapper.name,
      args: wrapper.args.to_string(),
      id: wrapper.id,
      metadata: wrapper.metadata,
    }
  }
}

#[napi(object)]
pub struct PluginToolResult {
  pub result: String,
  pub success: bool,
  pub error: Option<String>,
  pub metadata: HashMap<String, String>,
}

impl From<PluginToolResult> for ToolResultWrapper {
  fn from(result: PluginToolResult) -> Self {
    let value: serde_json::Value = serde_json::from_str(&result.result).unwrap_or_default();
    ToolResultWrapper {
      result: value,
      success: result.success,
      error: result.error,
      metadata: result.metadata,
    }
  }
}

impl From<ToolResultWrapper> for PluginToolResult {
  fn from(wrapper: ToolResultWrapper) -> Self {
    PluginToolResult {
      result: wrapper.result.to_string(),
      success: wrapper.success,
      error: wrapper.error,
      metadata: wrapper.metadata,
    }
  }
}

pub(crate) struct JSPlugin {
  name: String,
  on_llm_request_cb: Option<Arc<ThreadsafeFunction<JSAny>>>,
  on_llm_response_cb: Option<Arc<ThreadsafeFunction<JSAny>>>,
  on_tool_call_cb: Option<Arc<ThreadsafeFunction<JSAny>>>,
  on_tool_result_cb: Option<Arc<ThreadsafeFunction<JSAny>>>,
}

impl JSPlugin {
  pub fn new(
    name: String,
    on_llm_request: Option<ThreadsafeFunction<JSAny>>,
    on_llm_response: Option<ThreadsafeFunction<JSAny>>,
    on_tool_call: Option<ThreadsafeFunction<JSAny>>,
    on_tool_result: Option<ThreadsafeFunction<JSAny>>,
  ) -> Self {
    Self {
      name,
      on_llm_request_cb: on_llm_request.map(Arc::new),
      on_llm_response_cb: on_llm_response.map(Arc::new),
      on_tool_call_cb: on_tool_call.map(Arc::new),
      on_tool_result_cb: on_tool_result.map(Arc::new),
    }
  }

  fn call_js_callback(
    callback: &Arc<ThreadsafeFunction<JSAny>>,
    input: serde_json::Value,
  ) -> Option<serde_json::Value> {
    let (tx, rx) = std::sync::mpsc::channel::<Option<serde_json::Value>>();
    let tx_clone = tx.clone();

    callback.call_with_return_value(
      Ok(JSAny(input)),
      ThreadsafeFunctionCallMode::NonBlocking,
      move |result: std::result::Result<napi::Unknown<'_>, napi::Error>,
            _env|
            -> napi::Result<()> {
        match result {
          Ok(val) => {
            let json_str = val
              .coerce_to_string()
              .ok()
              .and_then(|s| s.into_utf8().ok())
              .and_then(|s| s.as_str().ok().map(|s| s.to_string()));

            let json_val = json_str.and_then(|s| {
              if s == "null" || s == "undefined" || s.is_empty() {
                None
              } else {
                serde_json::from_str(&s).ok()
              }
            });
            let _ = tx_clone.send(json_val);
          }
          Err(_) => {
            let _ = tx_clone.send(None);
          }
        }
        Ok(())
      },
    );

    rx.recv().unwrap_or(None)
  }
}

unsafe impl Send for JSPlugin {}
unsafe impl Sync for JSPlugin {}

#[async_trait]
impl AgentPlugin for JSPlugin {
  fn name(&self) -> &str {
    &self.name
  }

  async fn on_llm_request(&self, request: LlmRequestWrapper) -> Option<LlmRequestWrapper> {
    let callback = self.on_llm_request_cb.as_ref()?;
    let input = serde_json::json!({
        "model": request.model,
        "temperature": request.temperature,
        "max_tokens": request.max_tokens,
        "top_p": request.top_p,
        "top_k": request.top_k,
        "metadata": request.metadata,
    });

    let result = Self::call_js_callback(callback, input)?;

    let mut modified = request;
    if let Some(model) = result.get("model").and_then(|v| v.as_str()) {
      modified.model = model.to_string();
    }
    if let Some(temp) = result.get("temperature").and_then(|v| v.as_f64()) {
      modified.temperature = Some(temp as f32);
    }
    if let Some(max) = result.get("max_tokens").and_then(|v| v.as_u64()) {
      modified.max_tokens = Some(max as u32);
    }
    Some(modified)
  }

  async fn on_llm_response(&self, response: LlmResponseWrapper) -> Option<LlmResponseWrapper> {
    let callback = self.on_llm_response_cb.as_ref()?;

    let (content, tool_calls) = match &response {
      LlmResponseWrapper::OpenAI(rsp) => {
        let choice = rsp.choices.first();
        let content = choice.and_then(|c| c.message.content.clone());
        let tool_calls = choice.and_then(|c| c.message.tool_calls.clone());
        (content, tool_calls)
      }
    };

    let input = if let Some(ref tc) = tool_calls {
      serde_json::json!({
          "type": "ToolCall",
          "name": tc.first().and_then(|t| t.function.name.clone()).unwrap_or_default(),
          "args": tc.first().and_then(|t| t.function.arguments.clone()).unwrap_or_default(),
          "id": tc.first().map(|t| t.id.clone())
      })
    } else {
      serde_json::json!({"type": "Text", "content": content})
    };

    let result = Self::call_js_callback(callback, input)?;

    if result.get("type").and_then(|v| v.as_str()).is_some() {
      let LlmResponseWrapper::OpenAI(mut rsp) = response;
      if let Some(choice) = rsp.choices.first_mut() {
        if let Some(c) = result.get("content").and_then(|v| v.as_str()) {
          choice.message.content = Some(c.to_string());
        }
        if let Some(name) = result.get("name").and_then(|v| v.as_str()) {
          if let Some(ref mut tc) = choice.message.tool_calls {
            if let Some(first_tc) = tc.first_mut() {
              first_tc.function.name = Some(name.to_string());
            }
          }
        }
        if let Some(args) = result.get("args") {
          if let Some(ref mut tc) = choice.message.tool_calls {
            if let Some(first_tc) = tc.first_mut() {
              first_tc.function.arguments = Some(args.to_string());
            }
          }
        }
      }
      return Some(LlmResponseWrapper::OpenAI(rsp));
    }
    None
  }

  async fn on_tool_call(&self, tool_call: ToolCallWrapper) -> Option<ToolCallWrapper> {
    let callback = self.on_tool_call_cb.as_ref()?;
    let input = serde_json::json!({
        "name": tool_call.name,
        "args": tool_call.args,
        "id": tool_call.id,
        "metadata": tool_call.metadata,
    });

    let result = Self::call_js_callback(callback, input)?;

    let mut modified = tool_call;
    if let Some(name) = result.get("name").and_then(|v| v.as_str()) {
      modified.name = name.to_string();
    }
    if let Some(args) = result.get("args") {
      modified.args = args.clone();
    }
    Some(modified)
  }

  async fn on_tool_result(&self, tool_result: ToolResultWrapper) -> Option<ToolResultWrapper> {
    let callback = self.on_tool_result_cb.as_ref()?;
    let input = serde_json::json!({
        "result": tool_result.result,
        "success": tool_result.success,
        "error": tool_result.error,
        "metadata": tool_result.metadata,
    });

    let result = Self::call_js_callback(callback, input)?;

    let mut modified = tool_result;
    if let Some(res) = result.get("result") {
      modified.result = res.clone();
    }
    if let Some(success) = result.get("success").and_then(|v| v.as_bool()) {
      modified.success = success;
    }
    if let Some(error) = result.get("error") {
      modified.error = error.as_str().map(|s| s.to_string());
    }
    Some(modified)
  }
}

#[napi]
pub struct PluginRegistry {
  inner: InnerPluginRegistry,
}

#[napi]
impl PluginRegistry {
  #[napi(constructor)]
  pub fn new() -> Self {
    Self {
      inner: InnerPluginRegistry::new(),
    }
  }

  pub fn clone_inner(&self) -> InnerPluginRegistry {
    self.inner.clone()
  }

  pub fn inner(&self) -> &InnerPluginRegistry {
    &self.inner
  }

  #[napi]
  pub async fn clear(&self) {
    self.inner.clear().await;
  }
}
