// Performance benchmarks for backpressure components
//
// These benchmarks measure:
// - Rate limiter lock-free performance
// - Token bucket algorithm efficiency
// - Backpressure controller overhead
// - Concurrent access scalability

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::time::Duration;

use agent::streaming::backpressure::{BackpressureController, RateLimiter};

fn bench_rate_limiter_consecutive_calls(c: &mut Criterion) {
    let mut group = c.benchmark_group("rate_limiter_consecutive");

    for rate in [10.0, 100.0, 1000.0] {
        let limiter = RateLimiter::new(rate, rate);

        group.bench_with_input(
            BenchmarkId::from_parameter(rate),
            &limiter,
            |b, _limiter| {
                b.iter(|| {
                    black_box(_limiter.should_publish());
                });
            },
        );
    }

    group.finish();
}

fn bench_rate_limiter_burst_usage(c: &mut Criterion) {
    let limiter = RateLimiter::new(1000.0, 1000.0);

    c.bench_function("rate_limiter_burst_1000", |b| {
        b.iter(|| {
            let mut count = 0;
            for _ in 0..1000 {
                if black_box(&limiter).should_publish() {
                    count += 1;
                }
            }
            count
        });
    });
}

fn bench_backpressure_controller_update(c: &mut Criterion) {
    let mut group = c.benchmark_group("backpressure_controller");

    let controller = BackpressureController::new(100.0, 10, 50, Duration::from_millis(50));

    group.bench_function("should_publish", |b| {
        b.iter(|| {
            black_box(&controller).should_publish();
        });
    });

    group.bench_function("current_rate", |b| {
        b.iter(|| {
            black_box(&controller).current_rate();
        });
    });

    group.bench_function("current_load", |b| {
        b.iter(|| {
            black_box(&controller).current_load();
        });
    });

    group.finish();
}

fn bench_backpressure_controller_report_load(c: &mut Criterion) {
    let mut group = c.benchmark_group("backpressure_load_report");

    let loads = [0.1, 0.3, 0.5, 0.7, 0.9];

    for load in loads {
        let mut controller = BackpressureController::new(100.0, 10, 50, Duration::from_millis(50));

        group.bench_with_input(BenchmarkId::from_parameter(load), &load, |b, load| {
            b.iter(|| {
                black_box(&mut controller).report_bus_load(*load);
            });
        });
    }

    group.finish();
}

fn bench_rate_limiter_vs_no_limiter(c: &mut Criterion) {
    let limiter = RateLimiter::new(1000.0, 1000.0);

    let mut group = c.benchmark_group("overhead_comparison");

    group.bench_function("with_limiter", |b| {
        b.iter(|| {
            black_box(&limiter).should_publish();
        });
    });

    group.bench_function("without_limiter", |b| {
        b.iter(|| {
            black_box(true);
        });
    });

    group.finish();
}

fn bench_token_bucket_refill(c: &mut Criterion) {
    let limiter = RateLimiter::new(100.0, 0.0);

    c.bench_function("refill_internal", |b| {
        b.iter(|| {
            black_box(&limiter).current_rate();
        });
    });
}

criterion_group!(
    benches,
    bench_rate_limiter_consecutive_calls,
    bench_rate_limiter_burst_usage,
    bench_backpressure_controller_update,
    bench_backpressure_controller_report_load,
    bench_rate_limiter_vs_no_limiter,
    bench_token_bucket_refill,
);

criterion_main!(benches);
