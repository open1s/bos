use std::pin::Pin;
use std::sync::Arc;

use agent::tools::FunctionTool;
use agent::{Agent, AgentConfig, AgentRpcClient, LlmClient, LlmRequest, LlmResponse, StreamToken};
use async_trait::async_trait;
use bus::{Bus, BusConfig, Session};
use futures::{Stream, StreamExt};

struct MockLlm;

#[async_trait]
impl LlmClient for MockLlm {
    async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, react::llm::LlmError> {
        Ok(LlmResponse::Text("mock-complete".to_string()))
    }

    async fn stream_complete(
        &self,
        _req: LlmRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<StreamToken, react::llm::LlmError>> + Send>>,
        react::llm::LlmError,
    > {
        let stream = futures::stream::iter(vec![
            Ok(StreamToken::Text("chunk-1 ".to_string())),
            Ok(StreamToken::Text("chunk-2".to_string())),
            Ok(StreamToken::Done),
        ]);
        Ok(Box::pin(stream))
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "mock"
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Shared bus session for both caller and callee.
    let bus = Bus::from(BusConfig::default()).await;
    let session: Session = bus.clone().into();
    let session = Arc::new(session);

    // Callee agent with a simple tool.
    let mut callee = Agent::new(AgentConfig::default(), Arc::new(MockLlm));
    callee.try_add_tool(Arc::new(FunctionTool::new(
        "echo_json",
        "Echo input JSON",
        serde_json::json!({
            "type": "object",
            "additionalProperties": true
        }),
        |args| Ok(args.clone()),
    )))?;

    // Expose callee as RPC endpoint.
    let mut server = callee.as_callable_server("agent/rpc/demo", session.clone());
    server.start().await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Caller side: simplified typed client API.
    let client = AgentRpcClient::new("agent/rpc/demo", session);

    let listed = client.list().await?;
    println!("tool/list => {}", listed);

    let llm_out = client.llm_run("hello from llm_run").await?;
    println!("llm/run => {}", llm_out);

    println!("stream/run live events =>");
    let mut live_stream = client.stream_run_live("hello from stream_run_live").await?;
    while let Some(item) = live_stream.next().await {
        match item? {
            StreamToken::Text(t) => println!("  text: {:?}", t),
            StreamToken::ToolCall { name, args, id } => {
                println!("  tool_call: name={} id={:?} args={}", name, id, args)
            }
            StreamToken::Done => {
                println!("  done");
                break;
            }
        }
    }

    let stream_out = client.stream_run("hello from stream_run").await?;
    println!("stream/run => {}", stream_out);

    let call_out = client
        .call("echo_json", serde_json::json!({ "k": "v", "n": 420 }))
        .await?;
    println!("tool/call => {}", call_out);

    Ok(())
}
