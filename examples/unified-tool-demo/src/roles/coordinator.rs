use std::sync::Arc;
use std::time::Duration;
use zenoh::Session;
use agent::{UnifiedToolRegistry, ZenohRpcDiscovery, ToolDiscovery, ToolSource};
use agent::tools::BusToolClient;
use crate::get_local_tools;

pub async fn run_coordinator(session: Arc<Session>) -> anyhow::Result<()> {
    println!("[Coordinator] Starting...");

    let local_tools = get_local_tools();
    let tool_names: Vec<&str> = local_tools.iter().map(|t| t.name()).collect();
    println!("[Coordinator] Local tools: {:?}", tool_names);

    let mut registry = UnifiedToolRegistry::new()
        .with_zenoh_session(session.clone());

    for tool in local_tools {
        registry.registry_mut().register(tool)?;
    }

    println!("[Coordinator] Waiting for provider to start...");
    tokio::time::sleep(Duration::from_secs(2)).await;

    println!("[Coordinator] Discovering RPC tools from provider...");

    let rpc_discovery = ZenohRpcDiscovery::new(session.clone())
        .service_prefix("agent/demo");

    match rpc_discovery.discover_all().await {
        Ok(discovered_tools) => {
            let count = discovered_tools.len();
            println!("[Coordinator] Discovered {} RPC tools", count);
            for tool in &discovered_tools {
                println!("[Coordinator]   - {} (from {:?})", tool.name, tool.source);
            }

            for discovered in discovered_tools {
                if let ToolSource::ZenohRpc { service_name, .. } = discovered.source {
                    let client = BusToolClient::new(
                        session.clone(),
                        service_name,
                        discovered.name,
                    );
                    registry.registry_mut().register(std::sync::Arc::new(client))?;
                }
            }
        }
        Err(e) => {
            println!("[Coordinator] Discovery failed (provider may not be ready): {}", e);
        }
    }

    println!("[Coordinator] UnifiedToolRegistry initialized with {} tools",
        registry.registry().list().len());

    println!("[Coordinator] Registered tools:");
    for tool_name in registry.registry().list() {
        println!("  - {}", tool_name);
    }

    println!("\n[Coordinator] Testing tool execution...\n");

    if let Some(tool) = registry.registry().get("add") {
        let args = serde_json::json!({"a": 5.0, "b": 3.0});
        println!("[Coordinator] Calling local add(5, 3)...");
        match tool.execute(&args).await {
            Ok(result) => println!("[Coordinator] Result: {}", result),
            Err(e) => println!("[Coordinator] Error: {}", e),
        }
    }

    if let Some(tool) = registry.registry().get("multiply") {
        let args = serde_json::json!({"a": 4.0, "b": 7.0});
        println!("\n[Coordinator] Calling local multiply(4, 7)...");
        match tool.execute(&args).await {
            Ok(result) => println!("[Coordinator] Result: {}", result),
            Err(e) => println!("[Coordinator] Error: {}", e),
        }
    }

    if let Some(tool) = registry.registry().get("echo") {
        let args = serde_json::json!({"message": "hello", "repeat": 2});
        println!("\n[Coordinator] Calling function echo(hello x2)...");
        match tool.execute(&args).await {
            Ok(result) => println!("[Coordinator] Result: {}", result),
            Err(e) => println!("[Coordinator] Error: {}", e),
        }
    }

    println!("\n[Coordinator] Demo complete!");
    Ok(())
}