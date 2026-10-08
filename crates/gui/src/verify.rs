//! Live basic-function verification for the GUI's chat path.
//!
//! This exercises the exact pipeline the window uses — [`Settings`] →
//! [`caps::build_agent`] → [`Runner::spawn`] → streamed [`AgentEvent`]s —
//! against a real OpenAI-compatible endpoint, so a green run here is evidence
//! that the app can hold a conversation end to end.
//!
//! Ignored by default so the offline suite stays offline. Run it with:
//!
//! ```text
//! cargo test -p gui verify_live_chat -- --ignored --nocapture
//! ```
//!
//! Defaults point at the local broker; override with `BOS_GUI_VERIFY_BASE_URL`,
//! `BOS_GUI_VERIFY_API_KEY`, and `BOS_GUI_VERIFY_MODEL`.

use std::sync::Arc;
use std::time::Duration;

use crate::approval::ApprovalBroker;
use crate::caps::build_agent;
use crate::runner::{AgentEvent, Runner};
use crate::settings::Settings;

/// Environment override or fallback value for one connection setting.
fn env_or(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

/// One streaming turn: assert a non-empty, error-free assistant reply.
#[test]
#[ignore = "hits a live LLM endpoint"]
fn verify_live_chat_roundtrip() {
    let mut settings = Settings::from_config_value(&serde_json::Value::Null);
    settings.base_url = env_or("BOS_GUI_VERIFY_BASE_URL", "http://127.0.0.1:11436/v1");
    settings.api_key = env_or("BOS_GUI_VERIFY_API_KEY", "1234");
    settings.model = env_or("BOS_GUI_VERIFY_MODEL", "nvidia/z-ai/glm-5.3-flash");
    // Keep the verification turn pure chat: no shell, no file writes, no MCP.
    settings.bash_enabled = false;
    settings.file_tools_enabled = false;
    settings.require_approval = false;
    settings.mcp_servers.clear();
    settings.system_prompt =
        "You are a terse verification probe. Follow the user's instruction exactly.".to_string();

    let agent = build_agent(&settings, Arc::new(ApprovalBroker::default()));
    let mut runner = Runner::new();
    let rx = runner.spawn(agent, "Reply with exactly: PONG".to_string(), Vec::new());

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut usage: Option<(u64, u64)> = None;
    let mut error: Option<String> = None;

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        loop {
            match tokio::time::timeout(Duration::from_secs(120), rx.recv()).await {
                Ok(Ok(AgentEvent::Text(chunk))) => text.push_str(&chunk),
                Ok(Ok(AgentEvent::Reasoning(chunk))) => reasoning.push_str(&chunk),
                Ok(Ok(AgentEvent::Usage { prompt, completion })) => {
                    usage = Some((prompt, completion));
                }
                Ok(Ok(AgentEvent::Error(err))) => {
                    error = Some(err);
                    break;
                }
                Ok(Ok(AgentEvent::Done)) => break,
                Ok(Ok(AgentEvent::ToolCall(_))) => {}
                Ok(Err(_)) => break,
                Err(_) => {
                    error = Some("timed out waiting for the stream".to_string());
                    break;
                }
            }
        }
    });

    eprintln!("endpoint : {}", settings.base_url);
    eprintln!("model    : {}", settings.model);
    eprintln!("reasoning: {}", reasoning.trim());
    eprintln!("reply    : {}", text.trim());
    if let Some((prompt, completion)) = usage {
        eprintln!("usage    : prompt={prompt} completion={completion}");
    }

    assert!(
        error.is_none(),
        "stream failed: {}",
        error.unwrap_or_default()
    );
    let reply = text.trim();
    assert!(!reply.is_empty(), "assistant returned no text");
    assert!(
        reply.to_uppercase().contains("PONG"),
        "expected the reply to contain PONG, got: {reply}"
    );
}
