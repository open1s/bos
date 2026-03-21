use brainos_common::{setup_bus, setup_logging};
use bus::{RpcDiscovery, DiscoveryRegistry};
use tokio::signal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_logging()?;
    println!("╔══════════════════════════════════════╗");
    println!("║  Service Discovery Demo - Phase 1 Plan 02   ║");
    println!("╚══════════════════════════════════════╝\n");

    let session = setup_bus(None).await?;
    let discovery = RpcDiscovery::announce("demo-discovery");
    let registry = DiscoveryRegistry::new().session(session.clone());

    println!("Discovery registry demo:\n");
    let mut announcer = discovery.clone();
    announcer.init(session.clone()).await?;

    let services = registry.list_services().await?;
    println!("Found {} service(s) via discovery", services.len());

    let discovered = RpcDiscovery::discover("demo-rpc-service")
        .session(session.clone())
        .timeout(std::time::Duration::from_secs(2))
        .query()
        .await;
    match discovered {
        Ok(info) => println!("Found service: {} (v{})", info.service_name, info.version),
        Err(e) => println!("Service discovery timed out (expected if demo-rpc-service not running): {}", e),
    }

    println!("\nPress Ctrl+C to exit...");
    signal::ctrl_c().await?;
    println!("\nGoodbye!");
    Ok(())
}
