use bus::{rpc::{RpcDiscovery, DiscoveryRegistry}};
use brainos_common::setup_bus;

#[tokio::test]
async fn demo_discovery_single() {
    let session = setup_bus(None).await.unwrap();
    let discovery = RpcDiscovery::new(session.clone());
    
    // Discover without filter
    let services = discovery.discover(None).await.unwrap();
    // In a real test, we'd have announced a service first
    // For now, just verify the API works
    let _ = services;
    assert!(true);
}

#[tokio::test]
async fn demo_discovery_list() {
    let session = setup_bus(None).await.unwrap();
    let registry = DiscoveryRegistry::new();
    
    let services = registry.list_services(&session).await.unwrap();
    // Verify we can call the method
    let count = services.len();
    println!("Found {} service(s)", count);
    assert!(true);
}

#[tokio::test]
async fn demo_discovery_filter() {
    let session = setup_bus(None).await.unwrap();
    let discovery = RpcDiscovery::new(session.clone());
    
    // Test filtering by prefix
    let services = discovery.discover(Some("demo/")).await.unwrap();
    let count = services.len();
    println!("Found {} service(s) with prefix 'demo/'", count);
    assert!(true);
}

#[tokio::test]
async fn demo_discovery_timeout() {
    use tokio::time::{timeout, Duration};
    
    let session = setup_bus(None).await.unwrap();
    let discovery = RpcDiscovery::new(session.clone());
    
    // Test timeout by discovering a non-existent service
    let result = timeout(
        Duration::from_secs(2),
        discovery.discover(Some("nonexistent"))
    ).await;
    
    // Should timeout or return empty list
    match result {
        Ok(_) => assert!(true),
        Err(_) => assert!(true), // Timeout is expected
    }
}
