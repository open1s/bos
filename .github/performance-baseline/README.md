# Performance Baseline Artifact

This artifact contains baseline performance metrics for regression testing.

## How to Use

### Saving as Baseline
After establishing stable performance metrics:
1. Run full benchmark suite locally
2. Save results to `perf_results_baseline.txt`
3. Upload as GitHub Actions artifact named `baseline-perf`

### Updating Baseline
Intentional performance improvements should trigger baseline update:
1. Review improvement in pull request
2. Verify improvement is consistent and reproducible
3. Update expected thresholds in workflow file
4. Replace baseline artifact with new results

## Baseline Contents

- **message_serialization**: rkyv and JSON serialization benchmarks
- **Expected performance thresholds**:
  - rkyv_serialization/5000: < 30ns
  - rkyv_deserialization/100: < 25ns
  - json_serialization/5000: < 20µs

## Recent Baseline

**Last Updated**: TBD  
**Git SHA**: TBD  
**Benchmarks Run**:
- message_serialization (4 sizes)
- rkyv_serialization (100, 500, 1000, 5000 bytes)
- rkyv_deserialization (100, 500, 1000, 5000 bytes)
- json_serialization (100, 500, 1000, 5000 bytes)

## Regression Alerts

Alert triggers:
- **Time regression**: >10% increase vs baseline
- **Throughput regression**: >10% decrease vs baseline
- **Multiple regressions**: Alert even if each minor

## Performance Optimization History

| Date | Optimization | Improvement | SHA |
|------|------------|----------|-----|
| 2026-03-22 | ToolRegistry O(n) → O(1) | 10-200x | current |
| 2026-03-22 | Tool Schema Cache | 1.5-2x | current |
| 2026-03-22 | String Allocation Reducation | 1.2-1.5x | current |
| 2026-03-22 | Pre-allocation | 1.1-1.3x | current |

