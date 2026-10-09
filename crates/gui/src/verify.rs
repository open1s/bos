//! Live basic-function verification for the GUI's agent path.
//!
//! This exercises the exact pipeline the window uses — [`Settings`] →
//! [`caps::build_agent`] → [`Runner::spawn`] → streamed [`AgentEvent`]s —
//! against a real OpenAI-compatible endpoint, so a green run here is evidence
//! that the app can hold a conversation end to end (`verify_live_chat_roundtrip`),
//! survive a dead primary endpoint via the fallback chain
//! (`verify_live_failover_fallback`), drive its planning tool
//! (`verify_live_plan_tool`), fold a history into
//! a summary (`verify_live_compact_summary`), and draft an `AGENTS.md` for a
//! project tree (`verify_live_init`).
//!
//! Ignored by default so the offline suite stays offline. Run it with:
//!
//! ```text
//! cargo test -p gui verify_live -- --ignored --nocapture
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
    // The probe is read-only: no memory file is opened or written.
    settings.memory_enabled = false;
    settings.system_prompt =
        "You are a terse verification probe. Follow the user's instruction exactly.".to_string();

    let agent = build_agent(&settings, Arc::new(ApprovalBroker::default()));
    let mut runner = Runner::new();
    // The probe never compacts: budget 0 disables the send-side trimmer.
    let rx = runner.spawn(
        "__verify__",
        agent,
        "Reply with exactly: PONG".to_string(),
        Vec::new(),
        0,
    );

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
                Ok(Ok(AgentEvent::Compacted { .. })) => {}
                Ok(Ok(AgentEvent::Error(err))) => {
                    error = Some(err);
                    break;
                }
                Ok(Ok(AgentEvent::Done)) => break,
                Ok(Ok(AgentEvent::ToolCall(_))) => {}
                Ok(Ok(AgentEvent::ToolResult { .. })) => {}
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

/// One failover turn: the primary endpoint is dead, the fallback answers.
///
/// The primary points at a closed port (every attempt fails fast) while a
/// single enabled [`crate::settings::ProviderEntry`] carries the real broker
/// endpoint, so a PONG reply is end-to-end evidence that
/// [`crate::settings::Settings::agent_config`] wired the fallback chain and
/// the router actually failed over instead of giving up.
#[test]
#[ignore = "hits a live LLM endpoint"]
fn verify_live_failover_fallback() {
    let mut settings = Settings::from_config_value(&serde_json::Value::Null);
    // Dead primary: nothing listens on port 1, so the attempt errors instantly.
    settings.base_url = "http://127.0.0.1:1/v1".to_string();
    settings.api_key = "primary-is-dead".to_string();
    settings.model = env_or("BOS_GUI_VERIFY_MODEL", "nvidia/z-ai/glm-5.3-flash");
    settings.providers = vec![crate::settings::ProviderEntry {
        name: "broker".to_string(),
        base_url: env_or("BOS_GUI_VERIFY_BASE_URL", "http://127.0.0.1:11436/v1"),
        api_key: env_or("BOS_GUI_VERIFY_API_KEY", "1234"),
        model: env_or("BOS_GUI_VERIFY_MODEL", "nvidia/z-ai/glm-5.3-flash"),
        enabled: true,
    }];
    // Keep the verification turn pure chat: no shell, no file writes, no MCP.
    settings.bash_enabled = false;
    settings.file_tools_enabled = false;
    settings.require_approval = false;
    settings.mcp_servers.clear();
    settings.memory_enabled = false;
    settings.system_prompt =
        "You are a terse verification probe. Follow the user's instruction exactly.".to_string();

    let agent = build_agent(&settings, Arc::new(ApprovalBroker::default()));
    let mut runner = Runner::new();
    let rx = runner.spawn(
        "__verify__",
        agent,
        "Reply with exactly: PONG".to_string(),
        Vec::new(),
        0,
    );

    let mut text = String::new();
    let mut error: Option<String> = None;
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        loop {
            match tokio::time::timeout(Duration::from_secs(120), rx.recv()).await {
                Ok(Ok(AgentEvent::Text(chunk))) => text.push_str(&chunk),
                Ok(Ok(AgentEvent::Reasoning(_))) => {}
                Ok(Ok(AgentEvent::Usage { .. })) => {}
                Ok(Ok(AgentEvent::Compacted { .. })) => {}
                Ok(Ok(AgentEvent::Error(err))) => {
                    error = Some(err);
                    break;
                }
                Ok(Ok(AgentEvent::Done)) => break,
                Ok(Ok(AgentEvent::ToolCall(_))) => {}
                Ok(Ok(AgentEvent::ToolResult { .. })) => {}
                Ok(Err(_)) => break,
                Err(_) => {
                    error = Some("timed out waiting for the stream".to_string());
                    break;
                }
            }
        }
    });

    eprintln!("primary  : {}", settings.base_url);
    eprintln!(
        "fallback : {} ({})",
        settings.providers[0].base_url, settings.providers[0].model
    );
    eprintln!("reply    : {}", text.trim());

    assert!(
        error.is_none(),
        "failover chain failed end to end: {}",
        error.unwrap_or_default()
    );
    let reply = text.trim();
    assert!(!reply.is_empty(), "assistant returned no text");
    assert!(
        reply.to_uppercase().contains("PONG"),
        "expected the reply to contain PONG, got: {reply}"
    );
}

/// A live turn that must invoke `update_plan` and populate the shared store.
#[test]
#[ignore = "hits a live LLM endpoint"]
fn verify_live_plan_tool() {
    let mut settings = Settings::from_config_value(&serde_json::Value::Null);
    settings.base_url = env_or("BOS_GUI_VERIFY_BASE_URL", "http://127.0.0.1:11436/v1");
    settings.api_key = env_or("BOS_GUI_VERIFY_API_KEY", "1234");
    settings.model = env_or("BOS_GUI_VERIFY_MODEL", "nvidia/z-ai/glm-5.3-flash");
    // Pure planning turn: no shell, no file writes, no MCP, no memory file.
    settings.bash_enabled = false;
    settings.file_tools_enabled = false;
    settings.require_approval = false;
    settings.mcp_servers.clear();
    settings.memory_enabled = false;
    settings.system_prompt =
        "You are a terse verification probe. Follow the user's instruction exactly.".to_string();

    let agent = build_agent(&settings, Arc::new(ApprovalBroker::default()));
    let mut runner = Runner::new();
    let rx = runner.spawn(
        "__verify_plan__",
        Arc::clone(&agent),
        "Call the update_plan tool to replace the whole plan with exactly two \
         items: first {\"text\": \"first step\", \"status\": \"pending\"}, then \
         {\"text\": \"second step\", \"status\": \"in_progress\"}. \
         Then reply with exactly: PLANNED"
            .to_string(),
        Vec::new(),
        0,
    );

    let mut text = String::new();
    let mut plan_calls = 0usize;
    let mut plan_result: Option<(usize, u64)> = None;
    let mut error: Option<String> = None;

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        loop {
            match tokio::time::timeout(Duration::from_secs(120), rx.recv()).await {
                Ok(Ok(AgentEvent::Text(chunk))) => text.push_str(&chunk),
                Ok(Ok(AgentEvent::ToolCall(tool))) => {
                    if tool.name == "update_plan" {
                        plan_calls += 1;
                    }
                }
                // The ReAct loop reports each finished call; a live turn is the
                // end-to-end proof that the token reaches the GUI observer.
                Ok(Ok(AgentEvent::ToolResult { name, output, ms })) => {
                    if name == "update_plan" {
                        assert!(
                            !output.is_empty(),
                            "tool result must carry the tool's output"
                        );
                        plan_result = Some((output.len(), ms));
                    }
                }
                Ok(Ok(AgentEvent::Error(err))) => {
                    error = Some(err);
                    break;
                }
                Ok(Ok(AgentEvent::Done)) => break,
                Ok(Ok(_)) => {}
                Ok(Err(_)) => break,
                Err(_) => {
                    error = Some("timed out waiting for the stream".to_string());
                    break;
                }
            }
        }
    });

    eprintln!("plan calls: {plan_calls}");
    eprintln!("reply      : {}", text.trim());
    assert!(
        error.is_none(),
        "stream failed: {}",
        error.unwrap_or_default()
    );
    assert!(plan_calls > 0, "the model never invoked update_plan");
    let items = agent.plan_items();
    assert!(
        !items.is_empty(),
        "the shared plan store is empty after the turn"
    );
    eprintln!(
        "plan      : {:?}",
        items
            .iter()
            .map(|i| (i.text.as_str(), i.status))
            .collect::<Vec<_>>()
    );
}

/// A live summarization turn: `/compact`'s tool-less probe over a fixture.
///
/// Exercises [`crate::compact::summarize`] (probe settings + `Runner` +
/// deadline-bounded `collect`) and then the rewrite ([`crate::compact::apply`]),
/// so a green run proves manual compaction can fold a real history end to end.
#[test]
#[ignore = "hits a live LLM endpoint"]
fn verify_live_compact_summary() {
    let mut settings = Settings::from_config_value(&serde_json::Value::Null);
    settings.base_url = env_or("BOS_GUI_VERIFY_BASE_URL", "http://127.0.0.1:11436/v1");
    settings.api_key = env_or("BOS_GUI_VERIFY_API_KEY", "1234");
    settings.model = env_or("BOS_GUI_VERIFY_MODEL", "nvidia/z-ai/glm-5.3-flash");

    let fixture = vec![
        crate::session::ChatMessage::user(
            "Track the Project MANGO rollout; its budget is 12345 credits.",
        ),
        crate::session::ChatMessage::user("What is the budget again?"),
        {
            let mut m = crate::session::ChatMessage::assistant();
            m.text = "The Project MANGO budget is 12345 credits.".to_string();
            m
        },
        crate::session::ChatMessage::user("And the deadline is 2026-03-01."),
        {
            let mut m = crate::session::ChatMessage::assistant();
            m.text = "Noted: MANGO ships by 2026-03-01.".to_string();
            m
        },
        crate::session::ChatMessage::user("Summarize everything so far."),
    ];
    assert!(
        crate::compact::refusal(&fixture).is_none(),
        "fixture is compactable"
    );

    let summary = crate::compact::summarize(&settings, &fixture).expect("summarize");
    eprintln!("summary: {summary}");
    assert!(!summary.trim().is_empty(), "empty summary");
    assert!(
        summary.to_uppercase().contains("MANGO"),
        "identifier lost from the summary: {summary}"
    );
    assert!(
        summary.contains("12345"),
        "number lost from the summary: {summary}"
    );

    let mut record = crate::session::SessionRecord::new();
    record.messages = fixture;
    let (before, after) = crate::compact::apply(&mut record, summary).expect("apply");
    assert_eq!((before, after), (6, 2));
    assert_eq!(record.messages[0].role, crate::session::Role::User);
    assert!(record.messages[1].text.contains("MANGO"));
}

/// `/init` end to end: a tool-less probe drafts `AGENTS.md` from a shallow
/// project tree and the document lands in a throwaway workspace — never
/// this repository (the workspace refuses to overwrite an existing file).
///
/// Exercises [`crate::init::generate`] (probe settings + `Runner` +
/// deadline-bounded `collect`) and [`crate::init::write`], so a green run
/// proves AGENTS.md generation works against a real endpoint.
#[test]
#[ignore = "hits a live LLM endpoint"]
fn verify_live_init() {
    let mut settings = Settings::from_config_value(&serde_json::Value::Null);
    settings.base_url = env_or("BOS_GUI_VERIFY_BASE_URL", "http://127.0.0.1:11436/v1");
    settings.api_key = env_or("BOS_GUI_VERIFY_API_KEY", "1234");
    settings.model = env_or("BOS_GUI_VERIFY_MODEL", "nvidia/z-ai/glm-5.3-flash");

    let root = std::env::temp_dir().join(format!("bos-gui-verify-init-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("temp workspace");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"verify-demo\"\n",
    )
    .expect("fixture");
    std::fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").expect("fixture");

    let body = crate::init::generate(&settings, &root).expect("generation turn");
    eprintln!("AGENTS.md draft: {} chars", body.chars().count());
    assert!(
        body.chars().count() > 100,
        "suspiciously short draft: {body}"
    );

    let path = crate::init::write(&root, &body).expect("write");
    let saved = std::fs::read_to_string(&path).expect("read back");
    assert!(
        saved.contains("verify-demo") || saved.contains("src"),
        "draft is not project-specific: {saved}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
