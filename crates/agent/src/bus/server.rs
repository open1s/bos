use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bus::{Session, ZenohError, DEFAULT_CODEC};
use futures::StreamExt;
use tokio::task::JoinHandle;
use zenoh::query::Query as ZenohQuery;

use super::wire::*;
use crate::agent::Agent;
use crate::error::AgentError;
use react::ToolError;

pub(crate) async fn handle_rpc_request(
    agent: Arc<Agent>,
    req: AgentRpcRequest,
) -> AgentRpcResponse {
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
            let mut token_stream = agent.stream(&task);
            let mut text = String::new();
            let mut chunks: Vec<String> = Vec::new();
            loop {
                match token_stream.next().await {
                    Some(Ok(crate::StreamToken::Text(t))) => {
                        text.push_str(&t);
                        chunks.push(t);
                    }
                    Some(Ok(crate::StreamToken::ReasoningContent(_t))) => {
                        //SKIP
                    }
                    Some(Ok(crate::StreamToken::Usage(_))) => {
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
                    Some(Ok(crate::StreamToken::ToolResult { name, ms, .. })) => {
                        chunks.push(format!("[tool_result] name={} ms={}", name, ms));
                    }
                    Some(Ok(crate::StreamToken::Done)) => break,
                    Some(Ok(crate::StreamToken::Stopped)) => break,
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
                            "description": tool.description(),
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
                reg.execute(&tool_name, &tool_args)
            } else {
                Err(ToolError::Failed("tool registry not available".to_string()))
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

/// Expose an Agent instance as a bus callable endpoint.
pub struct AgentCallableServer {
    endpoint: String,
    session: Arc<Session>,
    agent: Arc<Agent>,
    started: AtomicBool,
    handle: Option<JoinHandle<()>>,
}

impl AgentCallableServer {
    /// Serve `agent` on `endpoint` over `session`.
    pub fn new(endpoint: impl Into<String>, session: Arc<Session>, agent: Arc<Agent>) -> Self {
        Self {
            endpoint: endpoint.into(),
            session,
            agent,
            started: AtomicBool::new(false),
            handle: None,
        }
    }

    /// The endpoint this server listens on.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Declare the queryable and begin serving; errors if already started.
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

pub(crate) async fn reply_response(
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

pub(crate) async fn handle_incoming_query(
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
        let mut token_stream = agent.stream(&task);
        let mut full_text = String::new();

        while let Some(item) = token_stream.next().await {
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
                Ok(crate::StreamToken::Usage(_)) => {
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
                Ok(crate::StreamToken::ToolResult { name, output, ms }) => {
                    reply_response(
                        &query,
                        endpoint,
                        AgentRpcResponse {
                            ok: true,
                            result: Some(serde_json::json!({
                                "event": {
                                    "type": "tool_result",
                                    "name": name,
                                    "output": output,
                                    "ms": ms
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
                Ok(crate::StreamToken::Stopped) => {
                    reply_response(
                        &query,
                        endpoint,
                        AgentRpcResponse {
                            ok: true,
                            result: Some(serde_json::json!({
                                "event": {
                                    "type": "stopped",
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
    use super::super::client::AgentRpcClient;
    use super::super::tool::AgentCallerTool;
    use super::super::transport::RpcTransport;
    use super::*;
    use crate::agent::agentic::{Agent, AgentConfig, LlmProvider};
    use crate::agent::context::{AgentReactContext, AgentSession};
    use crate::tools::FunctionTool;
    use async_trait::async_trait;
    use futures::Stream;
    use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice};
    use react::llm::{LlmClient, LlmError, LlmRequest, LlmResponse, StreamToken};
    use react::tool::Tool;
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
    impl LlmClient<AgentSession, AgentReactContext> for MockLlm {
        async fn complete(
            &self,
            _persona: Option<String>,
            _req: LlmRequest,
            _session: &mut AgentSession,
            _context: &mut AgentReactContext,
        ) -> Result<LlmResponse, LlmError> {
            Ok(make_text_response("mock-complete".to_string()))
        }

        async fn stream_complete(
            &self,
            _persona: Option<String>,
            _req: LlmRequest,
            _session: &mut AgentSession,
            _context: &mut AgentReactContext,
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

    fn make_llm_provider() -> LlmProvider {
        let mut provider = LlmProvider::new();
        provider.register_vendor("mock".to_string(), Box::new(MockLlm));
        provider
    }

    fn make_test_config() -> AgentConfig {
        AgentConfig {
            model: "mock/gpt-4".to_string(),
            ..AgentConfig::default()
        }
    }

    fn make_test_agent_with_echo_tool() -> Arc<Agent> {
        let mut agent = Agent::new(make_test_config(), Arc::new(make_llm_provider()));
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
        let agent = Arc::new(Agent::new(
            make_test_config(),
            Arc::new(make_llm_provider()),
        ));
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
        let mut token_stream = client.stream_run_live("hello").await.unwrap();
        while let Some(item) = token_stream.next().await {
            match item.unwrap() {
                crate::StreamToken::Text(t) => out.push_str(&t),
                crate::StreamToken::Done => break,
                crate::StreamToken::ToolCall { .. } => {}
                crate::StreamToken::ToolResult { .. } => {}
                crate::StreamToken::ReasoningContent(_) => {}
                crate::StreamToken::Usage(_) => {}
                crate::StreamToken::Stopped => break,
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
        let agent = Arc::new(Agent::new(
            make_test_config(),
            Arc::new(make_llm_provider()),
        ));
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

        let callee = make_test_agent_with_echo_tool();
        let mut server = AgentCallableServer::new("agent/rpc/test-callee", session.clone(), callee);
        server.start().await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let tool = AgentCallerTool::new("call_callee", "agent/rpc/test-callee", session);
        let result = tokio::task::spawn_blocking(move || {
            tool.run(&serde_json::json!({
                "method":"tool/call",
                "tool_name":"echo_json",
                "args":{"k":"v"}
            }))
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, serde_json::json!({"k":"v"}));
    }
}
