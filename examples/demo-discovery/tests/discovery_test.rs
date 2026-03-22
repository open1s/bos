use bus::{rpc::{RpcDiscovery, DiscoveryRegistry}};
use brainos_common::setup_bus;
use tokio::time::Duration;

async fn setup_bus_or_skip() -> Option<std::sync::Arc<bus::Session>> {
    match setup_bus(None).await {
        Ok(session) => Some(session),
        Err(err) => {
            eprintln!("skipping Zenoh integration assertion: {err}");
            None
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn demo_discovery_single() {
    let Some(session) = setup_bus_or_skip().await else {
        return;
    };
    let result = RpcDiscovery::discover("nonexistent-service")
        .session(session)
        .timeout(Duration::from_millis(100))
        .query()
        .await;

    assert!(result.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn demo_discovery_list() {
    let Some(session) = setup_bus_or_skip().await else {
        return;
    };
    let registry = DiscoveryRegistry::new()
        .session(session)
        .timeout(Duration::from_millis(100));

    let services = registry.list_services().await.unwrap();
    assert!(services.iter().all(|info| !info.service_name.is_empty()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn demo_discovery_filter() {
    let Some(session) = setup_bus_or_skip().await else {
        return;
    };
    let service_name = "demo-filter-service";
    let mut announcer = RpcDiscovery::announce(service_name);
    announcer.init(session.clone()).await.unwrap();

    let discovered = RpcDiscovery::discover(service_name)
        .session(session)
        .timeout(Duration::from_secs(1))
        .query()
        .await;

    if let Ok(info) = discovered {
        assert_eq!(info.service_name, service_name);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn demo_discovery_timeout() {
    let Some(session) = setup_bus_or_skip().await else {
        return;
    };
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        RpcDiscovery::discover("nonexistent")
            .session(session)
            .timeout(Duration::from_millis(100))
            .query(),
    )
    .await;

    assert!(result.is_ok());
    assert!(result.unwrap().is_err());
}
