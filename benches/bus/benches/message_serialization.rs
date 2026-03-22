use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

fn custom_criterion() -> Criterion {
    Criterion::default()
        .measurement_time(std::time::Duration::from_secs(10))
        .warm_up_time(std::time::Duration::from_secs(3))
        .sample_size(200)
}

fn bench_rkyv_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("rkyv_serialization");

    for size in [100, 500, 1000, 5000].iter() {
        let _data: Vec<u8> = (0..*size).map(|i| (i % 256) as u8).collect();
        group.throughput(Throughput::Bytes(*size as u64));

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, data| {
            b.iter(|| black_box(rkyv::to_bytes::<rkyv::rancor::Error>(black_box(data)).unwrap()));
        });
    }

    group.finish();
}

fn bench_rkyv_deserialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("rkyv_deserialization");

    for size in [100, 500, 1000, 5000].iter() {
        let data: Vec<u8> = (0..*size).map(|i| (i % 256) as u8).collect();
        let serialized = rkyv::to_bytes::<rkyv::rancor::Error>(&data).unwrap();

        group.throughput(Throughput::Bytes(*size as u64));

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| unsafe {
                black_box(
                    rkyv::from_bytes_unchecked::<Vec<u8>, rkyv::rancor::Error>(black_box(
                        &serialized.as_ref(),
                    ))
                    .unwrap(),
                )
            });
        });
    }

    group.finish();
}

fn bench_json_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_serialization");

    for size in [100, 500, 1000, 5000].iter() {
        let data: Vec<u8> = (0..*size).map(|i| (i % 256) as u8).collect();
        group.throughput(Throughput::Bytes(*size as u64));

        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| black_box(serde_json::to_vec(black_box(&data))));
        });
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = custom_criterion();
    targets = bench_rkyv_serialization, bench_rkyv_deserialization, bench_json_serialization
}
criterion_main!(benches);
