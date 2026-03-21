# Performance Regression Test Configuration

This file configures the performance regression testing system.

## Baseline Targets

### Message Serialization (message_serialization bench)

| Benchmark | Size | Expected Time | Warning Threshold |
|-----------|------|---------------|-------------------|
| rkyv_serialization | 100B | < 30 ns | +10% |
| rkyv_serialization | 500B | < 30 ns | +10% |
| rkyv_serialization | 1000B | < 30 ns | +10% |
| rkyv_serialization | 5000B | < 30 ns | +10% |
| json_serialization | 100B | < 400 ns | +15% |
| json_serialization | 500B | < 2.0 µs | +15% |
| json_serialization | 1000B | < 4.0 µs | +15% |
| json_serialization | 5000B | < 20 µs | +15% |

### Tool Registry (tool_registry bench)

| Benchmark | Tool Count | Expected Time (lookup) | Warning Threshold |
|-----------|-----------|---------------------|-------------------|
| tool_lookup/10 | 10 | < 20 ns | +15% |
| tool_lookup/50 | 50 | < 25 ns | +15% |
| tool_lookup/100 | 100 | < 30 ns | +20% |
| tool_lookup/200 | 200 | < 40 ns | +20% |

### LLM Client (llm_client bench)

| Benchmark | Operation | Expected Time | Warning Threshold |
|-----------|----------|---------------|-------------------|
| json_parsing/text_token | parse | < 500 ns | +15% |
| json_parsing/tool_call_token | parse | < 800 ns | +15% |
| request_building/simple | build | < 2 µs | +15% |
| request_building/with_tools | build | < 3 µs | +15% |
| token_parsing/text_token | parse | < 600 ns | +15% |

## Quick Test Configuration

For CI/CD environments where full benchmarks take too long:

```bash
# Use these settings for quick regression checks
cargo bench --bench message_serialization \
  --measurement-time=2 \
  --sample-size=30 \
  -- --noplot

# Run specific problematic benchmarks
cargo bench --bench tool_registry \
  --measurement-time=1 \
  --sample-size=20 \
  -- --noplot
```

## Baseline Management

### Creating Baseline
Save benchmark output as baseline:
```bash
cargo bench --bench message_serialization > baseline.txt
```

### Comparing Results
Use the provided script:
```bash
bash benchmarks_perf_test.sh
```

### Updating Baseline
When performance improves intentionally:
1. Review the improvement
2. Update expected thresholds in this file
3. Save new results as baseline:
   ```bash
   cp message_serialization_YYYYMMDD.txt baseline.txt
   ```

## Regression Alert

Action required when:
1. Any benchmark exceeds warning threshold (>10-20%)
2. Multiple benchmarks show degradation trend
3. Regression correlates with recent code changes

Investigation steps:
1. Check git log for recent changes
2. Identify the PR/commit causing regression
3. Revert or fix the offending change
4. Update baseline if improvement is intentional

## Performance Metrics to Monitor

- **Operation latency**: Mean, p50, p95, p99
- **Throughput**: Operations per second
- **Memory allocation**: Heap allocations, peak memory
- **CPU cycles**: CPU usage percentage

## Tools for Performance Analysis

- **criterion**: Statistical benchmarking framework
- **pprof**: CPU profiling & flamegraphs
- **valgrind/callgrind**: Call graph profiling
- **heaptrack**: Allocation tracking
- **tokio-console**: Async runtime diagnostics

## CI/CD Integration

Add to GitHub Actions or similar:

```yaml
name: Performance Tests
on: [pull_request, schedule]

jobs:
  performance:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      - run: cargo bench --bench message_serialization --measurement-time=2 --sample-size=30 -- --noplot
      - name: Compare Baseline
        run: benchmarks_perf_test.sh
```

## Frequency Recommendations

- **Full benchmarks**: Weekly or on release
- **Quick checks**: On every PR (quick mode)
- **Baseline updates**: When intentional improvements made
- **Regression investigation**: Immediately when alert triggered

