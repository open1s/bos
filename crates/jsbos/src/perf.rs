use napi_derive::napi;
use std::sync::Mutex;

/// Performance metrics collected across LLM calls.
/// All timing values are in microseconds.
#[derive(Debug, Clone)]
#[napi(object)]
pub struct PerfSnapshot {
  /// Number of LLM API calls completed
  pub llm_call_count: i64,
  pub total_wall_time_us: i64,
  pub avg_wall_time_us: i64,
  pub min_wall_time_us: i64,
  pub max_wall_time_us: i64,
  pub total_engine_time_us: i64,
  pub total_resilience_time_us: i64,
  pub rate_limit_waits: i64,
  pub total_rate_limit_wait_us: i64,
  pub circuit_trips: i64,
  pub llm_errors: i64,
  /// Number of tool invocations (not LLM calls)
  pub tool_invocation_count: i64,
  pub total_tool_time_us: i64,
  pub total_input_tokens: i64,
  pub total_output_tokens: i64,
}

#[derive(Debug, Default)]
#[allow(dead_code)]
struct MetricsInner {
  llm_call_count: i64,
  wall_times_us: Vec<i64>,
  total_engine_time_us: i64,
  total_resilience_time_us: i64,
  rate_limit_waits: i64,
  total_rate_limit_wait_us: i64,
  circuit_trips: i64,
  llm_errors: i64,
  tool_invocation_count: i64,
  total_tool_time_us: i64,
  total_input_tokens: i64,
  total_output_tokens: i64,
}

/// Thread-safe performance metrics collector.
#[derive(Debug, Default)]
pub struct PerformanceMetrics {
  inner: Mutex<MetricsInner>,
}

impl PerformanceMetrics {
  #[allow(dead_code)]
  pub fn new() -> Self {
    Self {
      inner: Mutex::new(MetricsInner::default()),
    }
  }

  /// Record a completed LLM call with timing breakdown.
  #[allow(dead_code)]
  pub fn record_call(
    &self,
    wall_time_us: i64,
    engine_time_us: i64,
    resilience_time_us: i64,
    input_tokens: i64,
    output_tokens: i64,
  ) {
    let mut inner = self.inner.lock().unwrap();
    inner.llm_call_count += 1;
    inner.wall_times_us.push(wall_time_us);
    inner.total_engine_time_us += engine_time_us;
    inner.total_resilience_time_us += resilience_time_us;
    inner.total_input_tokens += input_tokens;
    inner.total_output_tokens += output_tokens;
  }

  #[allow(dead_code)]
  pub fn record_rate_limit_wait(&self, wait_us: i64) {
    let mut inner = self.inner.lock().unwrap();
    inner.rate_limit_waits += 1;
    inner.total_rate_limit_wait_us += wait_us;
  }

  #[allow(dead_code)]
  pub fn record_circuit_trip(&self) {
    let mut inner = self.inner.lock().unwrap();
    inner.circuit_trips += 1;
  }

  /// Record an LLM error.
  #[allow(dead_code)]
  pub fn record_llm_error(&self) {
    let mut inner = self.inner.lock().unwrap();
    inner.llm_errors += 1;
  }

  /// Record a tool call with execution time.
  #[allow(dead_code)]
  pub fn record_tool_call(&self, time_us: i64) {
    let mut inner = self.inner.lock().unwrap();
    inner.tool_invocation_count += 1;
    inner.total_tool_time_us += time_us;
  }

  #[allow(dead_code)]
  pub fn snapshot(&self) -> PerfSnapshot {
    let inner = self.inner.lock().unwrap();
    let total_wall: i64 = inner.wall_times_us.iter().sum();
    PerfSnapshot {
      llm_call_count: inner.llm_call_count,
      total_wall_time_us: total_wall,
      avg_wall_time_us: if inner.llm_call_count > 0 {
        total_wall / inner.llm_call_count
      } else {
        0
      },
      min_wall_time_us: inner.wall_times_us.iter().copied().min().unwrap_or(0),
      max_wall_time_us: inner.wall_times_us.iter().copied().max().unwrap_or(0),
      total_engine_time_us: inner.total_engine_time_us,
      total_resilience_time_us: inner.total_resilience_time_us,
      rate_limit_waits: inner.rate_limit_waits,
      total_rate_limit_wait_us: inner.total_rate_limit_wait_us,
      circuit_trips: inner.circuit_trips,
      llm_errors: inner.llm_errors,
      tool_invocation_count: inner.tool_invocation_count,
      total_tool_time_us: inner.total_tool_time_us,
      total_input_tokens: inner.total_input_tokens,
      total_output_tokens: inner.total_output_tokens,
    }
  }

  /// Reset all metrics to zero.
  pub fn reset(&self) {
    let mut inner = self.inner.lock().unwrap();
    *inner = MetricsInner::default();
  }
}
