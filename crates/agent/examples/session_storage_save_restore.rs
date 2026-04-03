use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent::agent::agentic::AgentSession;
use agent::{
    Agent, AgentConfig, LlmClient, LlmRequest, LlmResponse, SessionConfig, SessionManager,
    StreamToken,
};
use async_trait::async_trait;
use futures::Stream;

struct EchoLlm;

#[async_trait]
impl LlmClient for EchoLlm {
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, react::llm::LlmError> {
        let last_user = req
            .context
            .conversations
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
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let base_dir = std::env::temp_dir().join(format!("bos_agent_sessions_demo_{ts}"));
    let config = SessionConfig {
        base_dir: base_dir.clone(),
        compression_enabled: true,
        ..Default::default()
    };
    let manager = SessionManager::new(config);
    let agent_id = "demo-storage-agent";

    // Build agent and start a session from persistent state.
    let agent = Agent::new(AgentConfig::default(), Arc::new(EchoLlm));
    let created = manager.create(agent_id.to_string()).await?;
    let mut session = AgentSession::restore_from_state(agent.clone(), created);

    let first = session.run("hello from storage session").await?;
    println!("First response: {first}");

    // Save latest state to storage.
    manager
        .update(agent_id, session.export_state(agent_id))
        .await?;
    println!("Session updated on disk");

    // Load from storage and continue.
    let loaded = manager.get(agent_id).await?;
    let agent = Agent::new(AgentConfig::default(), Arc::new(EchoLlm));
    let mut restored = AgentSession::restore_from_state(agent, loaded);
    let second = restored.run("continue after loading from disk").await?;
    println!("Restored response: {second}");

    manager
        .update(agent_id, restored.export_state(agent_id))
        .await?;
    let summaries = manager.list().await?;
    println!("Session summaries found: {}", summaries.len());

    manager.delete(agent_id).await?;
    if base_dir.exists() {
        tokio::fs::remove_dir_all(base_dir).await?;
    }
    Ok(())
}
