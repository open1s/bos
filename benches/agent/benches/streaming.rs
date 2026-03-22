//! Benchmarks for streaming token publishing
//!
//! These benchmarks measure performance of token publishing operations.

use agent::llm::StreamToken;
use agent::streaming::{SerializedToken, TokenBatch, TokenType};
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

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &_size| {
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

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &_size| {
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
            b.iter(|| black_box(Vec::<u8>::with_capacity(size)));
        });
    }

    group.finish();
}

fn bench_token_batch_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("token_batch_serialization");

    for token_count in [1usize, 8, 32, 128].iter() {
        let mut batch = TokenBatch::new();
        for i in 0..*token_count {
            batch.add(SerializedToken {
                task_id: format!("task-{}", i),
                token_type: if i % 3 == 0 {
                    TokenType::ToolCall
                } else {
                    TokenType::Text
                },
                tool_name: (i % 3 == 0).then(|| format!("tool_{}", i)),
                tool_args: (i % 3 == 0).then(|| {
                    serde_json::json!({
                        "city": "Shanghai",
                        "index": i,
                    })
                    .to_string()
                    .into_bytes()
                }),
                content: if i % 3 == 0 {
                    String::new()
                } else {
                    format!("token-{}", i)
                },
            });
        }

        group.bench_with_input(
            BenchmarkId::new("rkyv_serialize", token_count),
            token_count,
            |b, _| b.iter(|| black_box(batch.to_bytes_rkyv().unwrap())),
        );

        group.bench_with_input(
            BenchmarkId::new("json_serialize", token_count),
            token_count,
            |b, _| b.iter(|| black_box(batch.to_bytes().unwrap())),
        );

        let rkyv_bytes = batch.to_bytes_rkyv().unwrap();
        let json_bytes = batch.to_bytes().unwrap();

        group.bench_with_input(
            BenchmarkId::new("rkyv_deserialize", token_count),
            token_count,
            |b, _| b.iter(|| black_box(TokenBatch::from_bytes_rkyv(black_box(&rkyv_bytes)).unwrap())),
        );

        group.bench_with_input(
            BenchmarkId::new("json_deserialize", token_count),
            token_count,
            |b, _| b.iter(|| black_box(TokenBatch::from_bytes(black_box(&json_bytes)).unwrap())),
        );
    }

    group.finish();
}

fn bench_serialized_tool_call_conversion(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialized_tool_call_conversion");

    group.bench_function("from_stream_token", |b| {
        b.iter(|| {
            black_box(SerializedToken::from_stream_token(
                "task-1".to_string(),
                black_box(StreamToken::ToolCall {
                    name: "get_weather".to_string(),
                    args: serde_json::json!({
                        "city": "Shanghai",
                        "country": "CN",
                        "units": "metric",
                        "forecast_days": 3,
                    }),
                }),
            ))
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = custom_criterion();
    targets = bench_json_serialization, bench_json_deserialization, bench_string_allocation, bench_vec_allocation, bench_token_batch_serialization, bench_serialized_tool_call_conversion
}

criterion_main!(benches);
