use std::pin::Pin;
use std::sync::Arc;

use agent::agent::agentic::AgentSession;
use agent::{
    Agent, AgentConfig, LlmClient, LlmRequest, LlmResponse, SessionSerializer, StreamToken,
};
use async_trait::async_trait;
use futures::Stream;

struct EchoLlm;

#[async_trait]
impl LlmClient for EchoLlm {
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, react::llm::LlmError> {
        let last_user = req
            .messages
            .iter()
            .rev()
            .find_map(|m| match m {
                agent::OpenAiMessage::User { content } => Some(content.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "no-user-input".to_string());
        Ok(LlmResponse::Text(format!("Echo: {last_user}")))
    }

    async fn stream_complete(
        &self,
        req: LlmRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<StreamToken, react::llm::LlmError>> + Send>>,
        react::llm::LlmError,
    > {
        let text = match self.complete(req).await? {
            LlmResponse::Text(t) => t,
            _ => "unexpected".to_string(),
        };
        let stream =
            futures::stream::iter(vec![Ok(StreamToken::Text(text)), Ok(StreamToken::Done)]);
        Ok(Box::pin(stream))
    }

    fn supports_tools(&self) -> bool {
        false
    }

    fn provider_name(&self) -> &'static str {
        "echo-mock"
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Build an agent with a local mock LLM.
    let agent = Agent::new(AgentConfig::default(), Arc::new(EchoLlm));

    // Create a fresh session from an empty persisted state.
    let initial_state = SessionSerializer::new_state("demo-session".to_string());
    let mut session = AgentSession::restore_from_state(agent.clone(), initial_state);

    let first = session.run("Hello from original session").await?;
    println!("First response: {first}");

    // Save session to bytes (you can persist to file/db).
    let state = session.export_state("demo-session");
    let bytes = SessionSerializer::serialize(&state)?;
    println!("Saved session bytes: {}", bytes.len());

    // Restore session later from bytes.
    let restored_state = SessionSerializer::deserialize(&bytes)?;
    let mut restored = AgentSession::restore_from_state(agent, restored_state);

    let second = restored.run("Continue after restore").await?;
    println!("Restored response: {second}");

    Ok(())
}
