//! Benchmarks for streaming token publishing
//!
//! These benchmarks measure performance of token publishing operations.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::time::Duration;

fn custom_criterion() -> Criterion {
    Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(3))
        .sample_size(200)
}

fn bench_json_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_serialization");

    for size in [100, 500, 1000, 5000].iter() {
        let data: Vec<u8> = (0..*size).map(|i| (i % 256) as u8).collect();

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| black_box(serde_json::to_string(black_box(&data))));
        });
    }

    group.finish();
}

fn bench_json_deserialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_deserialization");

    for size in [100, 500, 1000, 5000].iter() {
        let data: Vec<u8> = (0..*size).map(|i| (i % 256) as u8).collect();
        let json = serde_json::to_string(&data).unwrap();

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| black_box(serde_json::from_str::<Vec<u8>>(black_box(&json))));
        });
    }

    group.finish();
}

fn bench_string_allocation(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_allocation");

    group.bench_function("small_string", |b| {
        b.iter(|| black_box("Hello, world!".to_string()));
    });

    group.bench_function("format_string", |b| {
        let base = "https://api.example.com".to_string();
        b.iter(|| black_box(format!("{}/chat/completions", black_box(&base))));
    });

    group.finish();
}

fn bench_vec_allocation(c: &mut Criterion) {
    let mut group = c.benchmark_group("vec_allocation");

    for size in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| black_box(Vec::<u8>::with_capacity(*size)));
        });
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = custom_criterion();
    targets = bench_json_serialization, bench_json_deserialization, bench_string_allocation, bench_vec_allocation
}

criterion_main!(benches);
