//! Benchmarks for rkyv-wrapped RPC payload formats.
//!
//! This compares the legacy `String`-backed JSON payload wrapper with the
//! optimized `Vec<u8>` wrapper used on the hot bus/RPC path.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rkyv::{Archive, Deserialize, Serialize};
use std::time::Duration;

#[derive(Archive, Serialize, Deserialize)]
struct StringJsonPayload {
    json: String,
}

#[derive(Archive, Serialize, Deserialize)]
struct BytesJsonPayload {
    json: Vec<u8>,
}

fn sample_value(size_hint: usize) -> serde_json::Value {
    serde_json::json!({
        "city": "Shanghai",
        "country": "CN",
        "units": "metric",
        "days": 3,
        "tags": (0..size_hint).map(|i| format!("tag-{}", i)).collect::<Vec<_>>(),
        "meta": {
            "source": "benchmark",
            "version": 1,
            "ok": true,
        }
    })
}

fn custom_criterion() -> Criterion {
    Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(3))
        .sample_size(200)
}

fn bench_rpc_payload_encoding(c: &mut Criterion) {
    let mut group = c.benchmark_group("rpc_payload_encoding");

    for size in [1usize, 8, 32, 128].iter() {
        let value = sample_value(*size);
        let json_string = serde_json::to_string(&value).unwrap();
        let json_bytes = serde_json::to_vec(&value).unwrap();

        group.bench_with_input(BenchmarkId::new("string_wrapper", size), size, |b, _| {
            b.iter(|| {
                black_box(
                    rkyv::to_bytes::<rkyv::rancor::Error>(&StringJsonPayload {
                        json: json_string.clone(),
                    })
                    .unwrap(),
                )
            });
        });

        group.bench_with_input(BenchmarkId::new("bytes_wrapper", size), size, |b, _| {
            b.iter(|| {
                black_box(
                    rkyv::to_bytes::<rkyv::rancor::Error>(&BytesJsonPayload {
                        json: json_bytes.clone(),
                    })
                    .unwrap(),
                )
            });
        });
    }

    group.finish();
}

fn bench_rpc_payload_decoding(c: &mut Criterion) {
    let mut group = c.benchmark_group("rpc_payload_decoding");

    for size in [1usize, 8, 32, 128].iter() {
        let value = sample_value(*size);
        let string_payload = rkyv::to_bytes::<rkyv::rancor::Error>(&StringJsonPayload {
            json: serde_json::to_string(&value).unwrap(),
        })
        .unwrap()
        .into_vec();
        let bytes_payload = rkyv::to_bytes::<rkyv::rancor::Error>(&BytesJsonPayload {
            json: serde_json::to_vec(&value).unwrap(),
        })
        .unwrap()
        .into_vec();

        group.bench_with_input(BenchmarkId::new("string_wrapper", size), size, |b, _| {
            b.iter(|| {
                let archived = black_box(
                    rkyv::from_bytes::<StringJsonPayload, rkyv::rancor::Error>(&string_payload)
                        .unwrap(),
                );
                black_box(serde_json::from_str::<serde_json::Value>(&archived.json).unwrap())
            });
        });

        group.bench_with_input(BenchmarkId::new("bytes_wrapper", size), size, |b, _| {
            b.iter(|| {
                let archived = black_box(
                    rkyv::from_bytes::<BytesJsonPayload, rkyv::rancor::Error>(&bytes_payload)
                        .unwrap(),
                );
                black_box(serde_json::from_slice::<serde_json::Value>(&archived.json).unwrap())
            });
        });
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = custom_criterion();
    targets = bench_rpc_payload_encoding, bench_rpc_payload_decoding
}

criterion_main!(benches);
