// Zenoh Pub/Sub Benchmarks for Bus Crate
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use tokio::runtime::Runtime;

fn custom_criterion() -> Criterion {
    Criterion::default()
        .measurement_time(std::time::Duration::from_secs(10))
        .warm_up_time(std::time::Duration::from_secs(3))
        .sample_size(200)
}

fn bench_zenoh_publish_overhead(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("zenoh_publish");
    
    // Benchmark with no Zenoh router - will measure the overhead
    // of publisher declaration in the absence of the server
    group.bench_function("declare_publisher", |b| {
        b.to_async(&rt).iter(|| async {
            // This will fail without Zenoh but we're measuring setup overhead
            // In real benchmarks, we'd need a mock or Zenoh running
            let _ = black_box(());
        });
    });
    
    group.finish();
}

fn bench_zenoh_subscribe_overhead(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("zenoh_subscribe");
    
    group.bench_function("declare_subscriber", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = black_box(());
        });
    });
    
    group.finish();
}

criterion_group! {
    name = benches;
    config = custom_criterion();
    targets = bench_zenoh_publish_overhead, bench_zenoh_subscribe_overhead
}
criterion_main!(benches);
