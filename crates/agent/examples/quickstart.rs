//! Minimal Rust quick start for the `agent` crate.
//!
//! Compiles with no network access; it only calls the model when
//! `OPENAI_API_KEY` is set.
//!
//! ```sh
//! OPENAI_API_KEY=sk-... cargo run -p agent --example quickstart
//! ```
use agent::{Agent, AgentConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();

    let agent = Agent::from_config(
        AgentConfig::default()
            .name("assistant")
            .model("openai/gpt-4o-mini")
            .api_key(api_key),
    );

    if std::env::var("OPENAI_API_KEY").is_err() {
        println!("Set OPENAI_API_KEY to send a real request.");
        return Ok(());
    }

    println!("{}", agent.run_simple("What is 2 + 2?").await?);
    Ok(())
}
