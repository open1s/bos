use std::sync::Arc;
use zenoh::Config;

use unified_tool_demo::roles::{run_coordinator, run_provider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    println!("=== Unified Tool Demo Started ===");
    println!("Spawning coordinator and provider tasks via tokio::spawn...\n");

    let session = Arc::new(zenoh::open(Config::default()).await.map_err(|e| anyhow::anyhow!("{}", e))?);
    println!("Connected to Zenoh\n");

    let session_coordinator = session.clone();
    let session_provider = session.clone();

    let coordinator_handle = tokio::spawn(async move {
        if let Err(e) = run_coordinator(session_coordinator).await {
            eprintln!("[Coordinator] Error: {}", e);
        }
    });

    let provider_handle = tokio::spawn(async move {
        if let Err(e) = run_provider(session_provider).await {
            eprintln!("[Provider] Error: {}", e);
        }
    });

    tokio::select! {
        _ = coordinator_handle => {}
        _ = provider_handle => {}
    }

    Ok(())
}