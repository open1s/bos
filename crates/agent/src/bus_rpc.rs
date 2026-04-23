use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bus::{Caller, Session, ZenohError, DEFAULT_CODEC};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use zenoh::query::{ConsolidationMode, Query as ZenohQuery};

use crate::agent::agentic::Agent;
use crate::error::{AgentError, ToolError};
use crate::tools::{Tool, ToolDescription};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentRpcRequest {
    method: String,
    task: Option<String>,
    tool_name: Option<String>,
    args: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentRpcResponse {
    ok: bool,
    result: Option<serde_json::Value>,
    error: Option<String>,
}

fn parse_request(payload: &str) -> Result<AgentRpcRequest, String> {
    serde_json::from_str(payload).map_err(|e| format!("invalid request JSON: {}", e))
}

fn encode_response(resp: AgentRpcResponse) -> Result<String, String> {
    serde_json::to_string(&resp).map_err(|e| format!("encode response failed: {}", e))
}

fn decode_response(payload: &str) -> Result<AgentRpcResponse, ToolError> {
    serde_json::from_str(payload)
        .map_err(|e| ToolError::ExecutionFailed(format!("invalid response JSON: {}", e)))
}

type RpcResponseStream = Pin<Box<dyn Stream<Item = Result<String, ToolError>> + Send>>;

#[async_trait]
trait RpcTransport: Send + Sync {
    async fn request(&self, payload: &str) -> Result<String, ToolError>;
    async fn request_stream(&self, payload: &str) -> Result<RpcResponseStream, ToolError>;
}

struct BusCallerTransport {
    caller: Caller,
    endpoint: String,
    session: Arc<Session>,
}

#[async_trait]
impl RpcTransport for BusCallerTransport {
    async fn request(&self, payload: &str) -> Result<String, ToolError> {
        self.caller
            .call::<String, String>(&payload.to_string())
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))
    }

    async fn request_stream(&self, payload: &str) -> Result<RpcResponseStream, ToolError> {
        let bytes = DEFAULT_CODEC
            .encode(&payload.to_string())
            .map_err(|e| ToolError::ExecutionFailed(format!("encode request failed: {}", e)))?;

        let replies = self
            .session
            .get(&self.endpoint)
            .payload(bytes)
            .consolidation(ConsolidationMode::None)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            while let Ok(reply) = replies.recv_async().await {
                match reply.result() {
                    Ok(sample) => {
                        let payload = sample.payload().to_bytes();
                        match DEFAULT_CODEC.decode::<String>(payload.as_ref()) {
                            Ok(decoded) => {
                                if tx.send(Ok(decoded)).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Err(ToolError::ExecutionFailed(format!(
                                        "decode response failed: {}",
                                        e
                                    ))))
                                    .await;
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(Err(ToolError::ExecutionFailed(e.to_string())))
                            .await;
                        break;
                    }
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}

/// Typed client for agent-to-agent RPC over bus.
///
/// This is the simplest user-facing API:
/// - `list()`
/// - `call(tool_name, args)`
/// - `llm_run(task)`
/// - `stream_run(task)`
#[derive(Clone)]
pub struct AgentRpcClient {
    endpoint: String,
    transport: Arc<dyn RpcTransport>,
}

impl AgentRpcClient {
    pub fn new(endpoint: impl Into<String>, session: Arc<Session>) -> Self {
        let endpoint = endpoint.into();
        let caller_endpoint = endpoint.clone();
        let stream_endpoint = endpoint.clone();
        let caller_session = session.clone();
        Self {
            endpoint: endpoint.clone(),
            transport: Arc::new(BusCallerTransport {
                caller: Caller::new(caller_endpoint, Some(caller_session)),
                endpoint: stream_endpoint,
                session,
            }),
        }
    }

    #[cfg(test)]
    fn with_transport(endpoint: impl Into<String>, transport: Arc<dyn RpcTransport>) -> Self {
        Self {
            endpoint: endpoint.into(),
            transport,
        }
    }

    async fn invoke_rpc(&self, req: AgentRpcRequest) -> Result<serde_json::Value, ToolError> {
        let payload = serde_json::to_string(&req)
            .map_err(|e| ToolError::ExecutionFailed(format!("encode request failed: {}", e)))?;
        let response_payload = self.transport.request(&payload).await?;
        let response = decode_response(&response_payload)?;
        if response.ok {
            Ok(response.result.unwrap_or(serde_json::Value::Null))
        } else {
            Err(ToolError::ExecutionFailed(
                response.error.unwrap_or_else(|| "remote error".to_string()),
            ))
        }
    }

    async fn invoke_rpc_stream(
        &self,
        req: AgentRpcRequest,
    ) -> Result<RpcResponseStream, ToolError> {
        let payload = serde_json::to_string(&req)
            .map_err(|e| ToolError::ExecutionFailed(format!("encode request failed: {}", e)))?;
        self.transport.request_stream(&payload).await
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn list(&self) -> Result<serde_json::Value, ToolError> {
        self.invoke_rpc(AgentRpcRequest {
            method: "tool/list".to_string(),
            task: None,
            tool_name: None,
            args: None,
        })
        .await
    }

    pub async fn call(
        &self,
        tool_name: impl Into<String>,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        self.invoke_rpc(AgentRpcRequest {
            method: "tool/call".to_string(),
            task: None,
            tool_name: Some(tool_name.into()),
            args: Some(args),
        })
        .await
    }

    pub async fn llm_run(&self, task: impl Into<String>) -> Result<serde_json::Value, ToolError> {
        self.invoke_rpc(AgentRpcRequest {
            method: "llm/run".to_string(),
            task: Some(task.into()),
            tool_name: None,
            args: None,
        })
        .await
    }

    /// Stream remote agent tokens over bus RPC.
    pub async fn stream_run_live(
        &self,
        task: impl Into<String>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<crate::StreamToken, ToolError>> + Send>>, ToolError>
    {
        let response_stream = self
            .invoke_rpc_stream(AgentRpcRequest {
                method: "stream/run".to_string(),
                task: Some(task.into()),
                tool_name: None,
                args: None,
            })
            .await?;

        let stream = async_stream::stream! {
            tokio::pin!(response_stream);
            while let Some(item) = response_stream.next().await {
                match item {
                    Ok(payload) => {
                        let response = match decode_response(&payload) {
                            Ok(v) => v,
                            Err(e) => {
                                yield Err(e);
                                break;
                            }
                        };

                        if !response.ok {
                            yield Err(ToolError::ExecutionFailed(
                                response.error.unwrap_or_else(|| "remote error".to_string()),
                            ));
                            break;
                        }

                        let Some(result) = response.result else {
                            continue;
                        };

                        if let Some(event) = result.get("event").and_then(|v| v.as_object()) {
                            match event.get("type").and_then(|v| v.as_str()) {
                                Some("text") => {
                                    if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                                        yield Ok(crate::StreamToken::Text(text.to_string()));
                                    }
                                }
                                Some("tool_call") => {
                                    let name = event
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default()
                                        .to_string();
                                    let args = event
                                        .get("args")
                                        .cloned()
                                        .unwrap_or_else(|| serde_json::json!({}));
                                    let id = event
                                        .get("id")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string());
                                    yield Ok(crate::StreamToken::ToolCall { name, args, id });
                                }
                                Some("done") => {
                                    yield Ok(crate::StreamToken::Done);
                                    break;
                                }
                                _ => {}
                            }
                            continue;
                        }

                        // Backward compatibility for older server responses.
                        if let Some(text) = result.get("text").and_then(|v| v.as_str()) {
                            yield Ok(crate::StreamToken::Text(text.to_string()));
                            yield Ok(crate::StreamToken::Done);
                            break;
                        }
                        if let Some(chunks) = result.get("chunks").and_then(|v| v.as_array()) {
                            for chunk in chunks {
                                if let Some(text) = chunk.as_str() {
                                    yield Ok(crate::StreamToken::Text(text.to_string()));
                                }
                            }
                            yield Ok(crate::StreamToken::Done);
                            break;
                        }
                    }
                    Err(e) => {
                        yield Err(e);
                        break;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    /// Run remote stream endpoint and aggregate response for compatibility.
    pub async fn stream_run(
        &self,
        task: impl Into<String>,
    ) -> Result<serde_json::Value, ToolError> {
        let mut text = String::new();
        let mut chunks = Vec::new();
        let mut stream = self.stream_run_live(task).await?;
        while let Some(item) = stream.next().await {
            match item? {
                crate::StreamToken::Text(t) => {
                    text.push_str(&t);
                    chunks.push(t);
                }
                crate::StreamToken::ReasoningContent(_t) => {
                    //SKIP
                }
                crate::StreamToken::ToolCall { name, args, id } => chunks.push(format!(
                    "[tool_call] name={} id={} args={}",
                    name,
                    id.unwrap_or_default(),
                    args
                )),
                crate::StreamToken::Done => break,
            }
        }
        Ok(serde_json::json!({ "text": text, "chunks": chunks }))
    }
}

fn tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "method": {
                "type": "string",
                "enum": ["llm/run", "stream/run", "tool/list", "tool/call"],
                "description": "Remote operation"
            },
            "task": {
                "type": "string",
                "description": "Required for method=llm/run or stream/run"
            },
            "tool_name": {
                "type": "string",
                "description": "Required for method=tool/call"
            },
            "args": {
                "type": "object",
                "description": "Arguments for remote tool call"
            }
        },
        "required": ["method"],
        "additionalProperties": false
    })
}

async fn handle_rpc_request(agent: Arc<Agent>, req: AgentRpcRequest) -> AgentRpcResponse {
    match req.method.as_str() {
        "llm/run" | "llm_run" => {
            let task = req.task.unwrap_or_default();
            match agent.run_simple(&task).await {
                Ok(text) => AgentRpcResponse {
                    ok: true,
                    result: Some(serde_json::json!({ "text": text })),
                    error: None,
                },
                Err(e) => AgentRpcResponse {
                    ok: false,
                    result: None,
                    error: Some(e.to_string()),
                },
            }
        }
        "stream/run" | "stream_run" => {
            let task = req.task.unwrap_or_default();
            let mut stream = agent.stream(&task);
            let mut text = String::new();
            let mut chunks: Vec<String> = Vec::new();
            loop {
                match stream.next().await {
                    Some(Ok(crate::StreamToken::Text(t))) => {
                        text.push_str(&t);
                        chunks.push(t);
                    }
                    Some(Ok(crate::StreamToken::ReasoningContent(_t))) => {
                        //SKIP
                    }
                    Some(Ok(crate::StreamToken::ToolCall { name, args, id })) => {
                        chunks.push(format!(
                            "[tool_call] name={} id={} args={}",
                            name,
                            id.unwrap_or_default(),
                            args
                        ));
                    }
                    Some(Ok(crate::StreamToken::Done)) => break,
                    Some(Err(e)) => {
                        return AgentRpcResponse {
                            ok: false,
                            result: None,
                            error: Some(e.to_string()),
                        };
                    }
                    None => break,
                }
            }

            AgentRpcResponse {
                ok: true,
                result: Some(serde_json::json!({
                    "text": text,
                    "chunks": chunks
                })),
                error: None,
            }
        }
        "tool/list" => {
            let tools = if let Some(reg) = agent.registry() {
                reg.iter()
                    .map(|(name, tool)| {
                        serde_json::json!({
                            "name": name,
                            "description": tool.description().short,
                            "parameters": tool.json_schema(),
                        })
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            AgentRpcResponse {
                ok: true,
                result: Some(serde_json::json!({ "tools": tools })),
                error: None,
            }
        }
        "tool/call" => {
            let tool_name = req.tool_name.unwrap_or_default();
            let tool_args = req.args.unwrap_or_else(|| serde_json::json!({}));
            let result = if let Some(reg) = agent.registry() {
                reg.execute(&tool_name, &tool_args).await
            } else {
                Err(ToolError::ExecutionFailed(
                    "tool registry not available".to_string(),
                ))
            };
            match result {
                Ok(value) => AgentRpcResponse {
                    ok: true,
                    result: Some(value),
                    error: None,
                },
                Err(e) => AgentRpcResponse {
                    ok: false,
                    result: None,
                    error: Some(e.to_string()),
                },
            }
        }
        _ => AgentRpcResponse {
            ok: false,
            result: None,
            error: Some("unsupported method".to_string()),
        },
    }
}

/// A local Tool that invokes another agent over the bus RPC path.
pub struct AgentCallerTool {
    tool_name: String,
    client: AgentRpcClient,
}

impl AgentCallerTool {
    pub fn new(
        tool_name: impl Into<String>,
        endpoint: impl Into<String>,
        session: Arc<Session>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            client: AgentRpcClient::new(endpoint, session),
        }
    }

    #[cfg(test)]
    fn with_transport(
        tool_name: impl Into<String>,
        endpoint: impl Into<String>,
        transport: Arc<dyn RpcTransport>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            client: AgentRpcClient::with_transport(endpoint, transport),
        }
    }

    /// List remote tools from the callee agent endpoint.
    pub async fn list(&self) -> Result<serde_json::Value, ToolError> {
        self.client.list().await
    }

    /// Call a specific remote tool on the callee agent endpoint.
    pub async fn call(
        &self,
        tool_name: impl Into<String>,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        self.client.call(tool_name, args).await
    }

    /// Run the remote agent using explicit llm/run method.
    pub async fn llm_run(&self, task: impl Into<String>) -> Result<serde_json::Value, ToolError> {
        self.client.llm_run(task).await
    }

    /// Run the remote agent using stream/run and return aggregated result payload.
    pub async fn stream_run(
        &self,
        task: impl Into<String>,
    ) -> Result<serde_json::Value, ToolError> {
        self.client.stream_run(task).await
    }

    /// Stream the remote agent response token-by-token using stream/run.
    pub async fn stream_run_live(
        &self,
        task: impl Into<String>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<crate::StreamToken, ToolError>> + Send>>, ToolError>
    {
        self.client.stream_run_live(task).await
    }
}

#[async_trait]
impl Tool for AgentCallerTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: format!(
                "Call remote agent via bus endpoint '{}' using method=llm/run/stream/run/tool/list/tool/call",
                self.client.endpoint()
            ),
            parameters: "JSON object".to_string(),
        }
    }

    fn json_schema(&self) -> serde_json::Value {
        tool_schema()
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let method = args
            .get("method")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                if args.get("tool_name").is_some() {
                    Some("tool/call".to_string())
                } else if args.get("task").is_some() {
                    Some("llm/run".to_string())
                } else {
                    Some("tool/list".to_string())
                }
            })
            .unwrap_or_else(|| "tool/list".to_string());

        match method.as_str() {
            "llm/run" | "llm_run" => {
                let task = args
                    .get("task")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::ExecutionFailed("missing 'task'".to_string()))?;
                self.llm_run(task.to_string()).await
            }
            "stream/run" | "stream_run" => {
                let task = args
                    .get("task")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::ExecutionFailed("missing 'task'".to_string()))?;
                self.stream_run(task.to_string()).await
            }
            "tool/list" => self.list().await,
            "tool/call" => {
                let tool_name = args
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::ExecutionFailed("missing 'tool_name'".to_string()))?;
                let call_args = args
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                self.call(tool_name.to_string(), call_args).await
            }
            _ => Err(ToolError::ExecutionFailed(
                "method must be one of: llm/run, stream/run, tool/list, tool/call".to_string(),
            )),
        }
    }
}

/// Expose an Agent instance as a bus callable endpoint.
pub struct AgentCallableServer {
    endpoint: String,
    session: Arc<Session>,
    agent: Arc<Agent>,
    started: AtomicBool,
    handle: Option<JoinHandle<()>>,
}

impl AgentCallableServer {
    pub fn new(endpoint: impl Into<String>, session: Arc<Session>, agent: Arc<Agent>) -> Self {
        Self {
            endpoint: endpoint.into(),
            session,
            agent,
            started: AtomicBool::new(false),
            handle: None,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn start(&mut self) -> Result<(), AgentError> {
        if self.started.swap(true, Ordering::Relaxed) {
            return Err(AgentError::Bus("server already started".to_string()));
        }

        let queryable = self
            .session
            .declare_queryable(&self.endpoint)
            .await
            .map_err(|e| AgentError::Bus(e.to_string()))?;

        let endpoint = self.endpoint.clone();
        let agent = self.agent.clone();
        self.handle = Some(tokio::spawn(async move {
            while let Ok(query) = queryable.recv_async().await {
                let endpoint = endpoint.clone();
                let agent = agent.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_incoming_query(query, &endpoint, agent).await {
                        log::warn!("agent RPC query handling failed: {}", e);
                    }
                });
            }
        }));

        Ok(())
    }
}

impl Drop for AgentCallableServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        self.started.store(false, Ordering::Relaxed);
    }
}

async fn reply_response(
    query: &ZenohQuery,
    endpoint: &str,
    response: AgentRpcResponse,
) -> Result<(), ZenohError> {
    let payload = encode_response(response).map_err(ZenohError::Query)?;
    let encoded = DEFAULT_CODEC
        .encode(&payload)
        .map_err(|e| ZenohError::Serialization(e.to_string()))?;
    query
        .reply(endpoint, encoded)
        .await
        .map_err(|e| ZenohError::Query(e.to_string()))
}

async fn handle_incoming_query(
    query: ZenohQuery,
    endpoint: &str,
    agent: Arc<Agent>,
) -> Result<(), ZenohError> {
    let Some(payload) = query.payload() else {
        query
            .reply_err("No payload in query".to_string())
            .await
            .map_err(|e| ZenohError::Query(e.to_string()))?;
        return Ok(());
    };

    let raw_request: String = DEFAULT_CODEC
        .decode(payload.to_bytes().as_ref())
        .map_err(|e| ZenohError::Serialization(e.to_string()))?;
    let req = parse_request(&raw_request).map_err(ZenohError::Query)?;

    if matches!(req.method.as_str(), "stream/run" | "stream_run") {
        let task = req.task.unwrap_or_default();
        let mut stream = agent.stream(&task);
        let mut full_text = String::new();

        while let Some(item) = stream.next().await {
            match item {
                Ok(crate::StreamToken::Text(t)) => {
                    full_text.push_str(&t);
                    reply_response(
                        &query,
                        endpoint,
                        AgentRpcResponse {
                            ok: true,
                            result: Some(serde_json::json!({
                                "event": {
                                    "type": "text",
                                    "text": t
                                }
                            })),
                            error: None,
                        },
                    )
                    .await?;
                }
                Ok(crate::StreamToken::ReasoningContent(_t)) => {
                    //SKIP
                }
                Ok(crate::StreamToken::ToolCall { name, args, id }) => {
                    reply_response(
                        &query,
                        endpoint,
                        AgentRpcResponse {
                            ok: true,
                            result: Some(serde_json::json!({
                                "event": {
                                    "type": "tool_call",
                                    "name": name,
                                    "args": args,
                                    "id": id
                                }
                            })),
                            error: None,
                        },
                    )
                    .await?;
                }
                Ok(crate::StreamToken::Done) => {
                    reply_response(
                        &query,
                        endpoint,
                        AgentRpcResponse {
                            ok: true,
                            result: Some(serde_json::json!({
                                "event": {
                                    "type": "done",
                                    "text": full_text
                                }
                            })),
                            error: None,
                        },
                    )
                    .await?;
                    return Ok(());
                }
                Err(e) => {
                    reply_response(
                        &query,
                        endpoint,
                        AgentRpcResponse {
                            ok: false,
                            result: None,
                            error: Some(e.to_string()),
                        },
                    )
                    .await?;
                    return Ok(());
                }
            }
        }

        reply_response(
            &query,
            endpoint,
            AgentRpcResponse {
                ok: true,
                result: Some(serde_json::json!({
                    "event": {
                        "type": "done",
                        "text": full_text
                    }
                })),
                error: None,
            },
        )
        .await?;
        return Ok(());
    }

    let response = handle_rpc_request(agent, req).await;
    reply_response(&query, endpoint, response).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::FunctionTool;
    use futures::Stream;
    use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice};
    use react::llm::{LlmClient, LlmError, LlmRequest, LlmResponse, StreamToken};
    use std::pin::Pin;
    use std::sync::Mutex;

    struct MockLlm;

    fn make_text_response(content: String) -> LlmResponse {
        LlmResponse::OpenAI(ChatCompletionResponse {
            id: "test-mock".to_string(),
            object: "chat.completion".to_string(),
            created: 1234567890,
            model: "mock-model".to_string(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some(content),
                    tool_calls: None,
                    function_call: None,
                    reasoning_content: None,
                    extra: serde_json::Value::Object(serde_json::Map::new()),
                },
                finish_reason: Some("stop".to_string()),
                stop_reason: None,
                logprobs: None,
            }],
            usage: None,
            system_fingerprint: None,
            nvext: None,
        })
    }

    #[async_trait]
    impl LlmClient for MockLlm {
        async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, LlmError> {
            Ok(make_text_response("mock-complete".to_string()))
        }

        async fn stream_complete(
            &self,
            _req: LlmRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, LlmError>
        {
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamToken::Text("s1".to_string())),
                Ok(StreamToken::Text("s2".to_string())),
                Ok(StreamToken::Done),
            ])))
        }

        fn supports_tools(&self) -> bool {
            false
        }

        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    struct MockTransport {
        seen_payloads: Arc<Mutex<Vec<String>>>,
        response_payload: String,
        response_stream_payloads: Option<Vec<String>>,
    }

    #[async_trait]
    impl RpcTransport for MockTransport {
        async fn request(&self, payload: &str) -> Result<String, ToolError> {
            self.seen_payloads.lock().unwrap().push(payload.to_string());
            Ok(self.response_payload.clone())
        }

        async fn request_stream(&self, payload: &str) -> Result<RpcResponseStream, ToolError> {
            self.seen_payloads.lock().unwrap().push(payload.to_string());
            let responses = self
                .response_stream_payloads
                .clone()
                .unwrap_or_else(|| vec![self.response_payload.clone()]);
            Ok(Box::pin(tokio_stream::iter(
                responses.into_iter().map(Ok::<String, ToolError>),
            )))
        }
    }

    fn make_test_agent_with_echo_tool() -> Arc<Agent> {
        let mut agent = Agent::new(crate::AgentConfig::default(), Arc::new(MockLlm));
        agent
            .try_add_tool(Arc::new(FunctionTool::new(
                "echo_json",
                "echo args",
                serde_json::json!({"type":"object"}),
                |args| Ok(args.clone()),
            )))
            .unwrap();
        Arc::new(agent)
    }

    #[tokio::test]
    async fn test_handle_rpc_tool_list() {
        let agent = make_test_agent_with_echo_tool();
        let list_resp = handle_rpc_request(
            agent,
            AgentRpcRequest {
                method: "tool/list".to_string(),
                task: None,
                tool_name: None,
                args: None,
            },
        )
        .await;
        assert!(list_resp.ok);
        let tools = list_resp
            .result
            .as_ref()
            .and_then(|v| v.get("tools"))
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(tools
            .iter()
            .any(|t| t.get("name") == Some(&serde_json::json!("echo_json"))));
    }

    #[tokio::test]
    async fn test_handle_rpc_tool_call() {
        let agent = make_test_agent_with_echo_tool();
        let call_resp = handle_rpc_request(
            agent,
            AgentRpcRequest {
                method: "tool/call".to_string(),
                task: None,
                tool_name: Some("echo_json".to_string()),
                args: Some(serde_json::json!({"k":"v"})),
            },
        )
        .await;
        assert!(call_resp.ok);
        assert_eq!(call_resp.result.unwrap(), serde_json::json!({"k":"v"}));
    }

    #[tokio::test]
    async fn test_handle_rpc_llm_run() {
        let agent = Arc::new(Agent::new(crate::AgentConfig::default(), Arc::new(MockLlm)));
        let resp = handle_rpc_request(
            agent,
            AgentRpcRequest {
                method: "llm/run".to_string(),
                task: Some("hello-llm".to_string()),
                tool_name: None,
                args: None,
            },
        )
        .await;
        assert!(resp.ok);
        let text = resp
            .result
            .as_ref()
            .and_then(|v| v.get("text"))
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(text, "mock-complete");
    }

    #[tokio::test]
    async fn test_handle_rpc_tool_list_and_tool_call() {
        let agent = make_test_agent_with_echo_tool();

        let list_resp = handle_rpc_request(
            agent.clone(),
            AgentRpcRequest {
                method: "tool/list".to_string(),
                task: None,
                tool_name: None,
                args: None,
            },
        )
        .await;
        assert!(list_resp.ok);
        let tools = list_resp
            .result
            .as_ref()
            .and_then(|v| v.get("tools"))
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(tools
            .iter()
            .any(|t| t.get("name") == Some(&serde_json::json!("echo_json"))));

        let call_resp = handle_rpc_request(
            agent,
            AgentRpcRequest {
                method: "tool/call".to_string(),
                task: None,
                tool_name: Some("echo_json".to_string()),
                args: Some(serde_json::json!({"k":"v"})),
            },
        )
        .await;
        assert!(call_resp.ok);
        assert_eq!(call_resp.result.unwrap(), serde_json::json!({"k":"v"}));
    }

    #[tokio::test]
    async fn test_agent_caller_tool_list_method() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let transport = Arc::new(MockTransport {
            seen_payloads: seen.clone(),
            response_payload: serde_json::json!({
                "ok": true,
                "result": {"tools":[]},
                "error": null
            })
            .to_string(),
            response_stream_payloads: None,
        });
        let tool = AgentCallerTool::with_transport("remote", "agent/rpc/x", transport);
        let result = tool.list().await.unwrap();
        assert_eq!(result, serde_json::json!({"tools":[]}));

        let payloads = seen.lock().unwrap();
        let req: serde_json::Value = serde_json::from_str(&payloads[0]).unwrap();
        assert_eq!(req["method"], "tool/list");
    }

    #[tokio::test]
    async fn test_agent_caller_tool_call_method() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let transport = Arc::new(MockTransport {
            seen_payloads: seen.clone(),
            response_payload: serde_json::json!({
                "ok": true,
                "result": {"k":"v"},
                "error": null
            })
            .to_string(),
            response_stream_payloads: None,
        });
        let tool = AgentCallerTool::with_transport("remote", "agent/rpc/x", transport);
        let result = tool
            .call("echo_json", serde_json::json!({"k":"v"}))
            .await
            .unwrap();
        assert_eq!(result, serde_json::json!({"k":"v"}));

        let payloads = seen.lock().unwrap();
        let req: serde_json::Value = serde_json::from_str(&payloads[0]).unwrap();
        assert_eq!(req["method"], "tool/call");
        assert_eq!(req["tool_name"], "echo_json");
    }

    #[tokio::test]
    async fn test_agent_caller_tool_llm_run_method() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let transport = Arc::new(MockTransport {
            seen_payloads: seen.clone(),
            response_payload: serde_json::json!({
                "ok": true,
                "result": {"text":"ok"},
                "error": null
            })
            .to_string(),
            response_stream_payloads: None,
        });
        let tool = AgentCallerTool::with_transport("remote", "agent/rpc/x", transport);
        let result = tool.llm_run("hello").await.unwrap();
        assert_eq!(result, serde_json::json!({"text":"ok"}));

        let payloads = seen.lock().unwrap();
        let req: serde_json::Value = serde_json::from_str(&payloads[0]).unwrap();
        assert_eq!(req["method"], "llm/run");
        assert_eq!(req["task"], "hello");
    }

    #[tokio::test]
    async fn test_agent_rpc_client_stream_run_live() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stream_payloads = vec![
            serde_json::json!({
                "ok": true,
                "result": {"event": {"type": "text", "text": "s1"}},
                "error": null
            })
            .to_string(),
            serde_json::json!({
                "ok": true,
                "result": {"event": {"type": "text", "text": "s2"}},
                "error": null
            })
            .to_string(),
            serde_json::json!({
                "ok": true,
                "result": {"event": {"type": "done", "text": "s1s2"}},
                "error": null
            })
            .to_string(),
        ];

        let transport = Arc::new(MockTransport {
            seen_payloads: seen.clone(),
            response_payload: serde_json::json!({
                "ok": true,
                "result": serde_json::Value::Null,
                "error": null
            })
            .to_string(),
            response_stream_payloads: Some(stream_payloads),
        });
        let client = AgentRpcClient::with_transport("agent/rpc/x", transport);

        let mut out = String::new();
        let mut stream = client.stream_run_live("hello").await.unwrap();
        while let Some(item) = stream.next().await {
            match item.unwrap() {
                crate::StreamToken::Text(t) => out.push_str(&t),
                crate::StreamToken::Done => break,
                crate::StreamToken::ToolCall { .. } => {}
                crate::StreamToken::ReasoningContent(_) => {}
            }
        }
        assert_eq!(out, "s1s2");

        let payloads = seen.lock().unwrap();
        let req: serde_json::Value = serde_json::from_str(&payloads[0]).unwrap();
        assert_eq!(req["method"], "stream/run");
        assert_eq!(req["task"], "hello");
    }

    #[tokio::test]
    async fn test_handle_rpc_stream_run() {
        let agent = Arc::new(Agent::new(crate::AgentConfig::default(), Arc::new(MockLlm)));
        let resp = handle_rpc_request(
            agent,
            AgentRpcRequest {
                method: "stream/run".to_string(),
                task: Some("hello".to_string()),
                tool_name: None,
                args: None,
            },
        )
        .await;
        assert!(resp.ok);
        let text = resp
            .result
            .as_ref()
            .and_then(|v| v.get("text"))
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(text, "s1s2");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_agent_to_agent_tool_call_via_bus() {
        let config = zenoh::Config::default();
        let session = Arc::new(zenoh::open(config).await.unwrap());

        let llm = Arc::new(react::llm::vendor::OpenAiClient::new(
            "https://api.openai.com/v1".to_string(),
            "gpt-4".to_string(),
            "dummy".to_string(),
        ));
        let mut callee = Agent::new(crate::AgentConfig::default(), llm);
        callee
            .try_add_tool(Arc::new(FunctionTool::new(
                "echo_json",
                "echo args",
                serde_json::json!({"type":"object"}),
                |args| Ok(args.clone()),
            )))
            .unwrap();

        let mut server =
            AgentCallableServer::new("agent/rpc/test-callee", session.clone(), Arc::new(callee));
        server.start().await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let tool = AgentCallerTool::new("call_callee", "agent/rpc/test-callee", session);
        let result = tool
            .execute(&serde_json::json!({
                "method":"tool/call",
                "tool_name":"echo_json",
                "args":{"k":"v"}
            }))
            .await
            .unwrap();
        assert_eq!(result, serde_json::json!({"k":"v"}));
    }
}
