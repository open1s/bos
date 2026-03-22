use std::sync::Arc;
use agent::llm::{OpenAiClient, OpenAiMessage, LlmRequest, LlmClient as LlmClientTrait};
use agent::streaming::TokenPublisherWrapper;
use anyhow::Result;
use brainos_common::{setup_bus, setup_logging};
use clap::Parser;
use tokio_stream::StreamExt;
use tokio::signal;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    topic_prefix: Option<String>,

    #[arg(long)]
    agent_id: Option<String>,

    #[arg(long)]
    prompt: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_logging()?;

    let args = Args::parse();
    let session = setup_bus(None).await?;

    println!("╔══════════════════════════════════════╗");
    println!("║  Streaming Demo - Phase 4 Plan 01    ║");
    println!("╚══════════════════════════════════════╝\n");

    let openai_key = std::env::var("OPENAI_API_KEY")
        .expect("OPENAI_API_KEY environment variable must be set");

    let base_url = std::env::var("OPENAI_API_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

    let model = std::env::var("OPENAI_MODEL")
        .unwrap_or_else(|_| "gpt-4o".to_string());

    let topic_prefix = args.topic_prefix
        .unwrap_or_else(|| "demo/streaming".to_string());

    let agent_id = args.agent_id
        .unwrap_or_else(|| "streaming-agent".to_string());

    let prompt = args.prompt
        .unwrap_or_else(|| "Write a 3-line poem about coding".to_string());

    println!("✓ Agent ID: {}", agent_id);

    let publisher = Arc::new(TokenPublisherWrapper::new(
        session.clone(),
        agent_id.clone(),
        topic_prefix.clone(),
    ));

    println!("✓ Created TokenPublisher on topic: {}/{}/tokens/stream",
        topic_prefix, agent_id);

    let llm_client = OpenAiClient::new(base_url, openai_key);

    println!("✓ Created OpenAiClient (model: {})\n", model);

    println!("✓ Created OpenAiClient (model: {})\n", model);

    println!("Prompt: {}\n", prompt);
    println!("{}", "=".repeat(50));
    println!("Streaming tokens to Zenoh bus...");
    println!("{}", "=".repeat(50));
    println!();

    let task_id = uuid::Uuid::new_v4().to_string();

    let req = LlmRequest {
        model: model.clone(),
        messages: vec![OpenAiMessage::User { content: prompt }],
        tools: None,
        temperature: 0.7,
        max_tokens: Some(1000),
    };

    let mut stream = llm_client.stream_complete(req);

    let mut token_count = 0;

    while let Some(token_result) = stream.next().await {
        match token_result {
            Ok(token) => {
                match &token {
                    agent::llm::StreamToken::Text(content) => {
                        token_count += 1;
                        println!("[{}] Token {}: {}", task_id, token_count, content);
                    }
                    agent::llm::StreamToken::ToolCall { name, .. } => {
                        println!("[{}] Tool call: {}", task_id, name);
                    }
                    agent::llm::StreamToken::Done => {
                        println!("[{}] Stream complete", task_id);
                    }
                }

                publisher.publish_token(&task_id, token).await?;
            }
            Err(e) => {
                eprintln!("✗ Streaming error: {}", e);
                break;
            }
        }
    }

    publisher.flush().await?;
    println!("\n✓ Stream complete, flushed batch ({} tokens published)", token_count);

    println!("\nPress Ctrl+C to exit...");
    signal::ctrl_c().await?;
    println!("\nGoodbye!");

    Ok(())
}
