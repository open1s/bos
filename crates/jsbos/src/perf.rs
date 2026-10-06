use napi_derive::napi;

/// Performance metrics collected across LLM calls.
/// All timing values are in microseconds.
#[derive(Debug, Clone)]
#[napi(object)]
pub struct PerfSnapshot {
  /// Number of LLM API calls completed
  pub llm_call_count: i64,
  /// Total wall-clock time across calls.
  pub total_wall_time_us: i64,
  /// Average wall-clock time per call.
  pub avg_wall_time_us: i64,
  /// Minimum wall-clock time.
  pub min_wall_time_us: i64,
  /// Maximum wall-clock time.
  pub max_wall_time_us: i64,
  /// Total engine time.
  pub total_engine_time_us: i64,
  /// Total time spent in resilience layers.
  pub total_resilience_time_us: i64,
  /// Number of rate-limit waits.
  pub rate_limit_waits: i64,
  /// Total time spent waiting on rate limits.
  pub total_rate_limit_wait_us: i64,
  /// Number of circuit breaker trips.
  pub circuit_trips: i64,
  /// Number of LLM errors.
  pub llm_errors: i64,
  /// Number of tool invocations (not LLM calls)
  pub tool_invocation_count: i64,
  /// Total tool execution time.
  pub total_tool_time_us: i64,
  /// Total input tokens.
  pub total_input_tokens: i64,
  /// Total output tokens.
  pub total_output_tokens: i64,
}
