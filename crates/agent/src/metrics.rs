use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct CallMetrics {
  pub call_count: u64,
  pub total_wall_time: Duration,
  pub total_engine_time: Duration,
  pub total_resilience_time: Duration,
  pub rate_limit_waits: u64,
  pub total_rate_limit_wait: Duration,
  pub circuit_trips: u64,
  pub llm_errors: u64,
  pub tool_call_count: u64,
  pub total_tool_time: Duration,
  pub total_input_tokens: u64,
  pub total_output_tokens: u64,
}

#[derive(Debug)]
pub struct MetricsCollector {
  inner: Mutex<CallMetrics>,
}

impl MetricsCollector {
  pub fn new() -> Self {
    Self {
      inner: Mutex::new(CallMetrics::default()),
    }
  }

  pub fn record_call(
    &self,
    wall_time: Duration,
    engine_time: Duration,
    resilience_time: Duration,
    input_tokens: u64,
    output_tokens: u64,
  ) {
    let mut m = self.inner.lock().unwrap();
    m.call_count += 1;
    m.total_wall_time += wall_time;
    m.total_engine_time += engine_time;
    m.total_resilience_time += resilience_time;
    m.total_input_tokens += input_tokens;
    m.total_output_tokens += output_tokens;
  }

  pub fn record_rate_limit_wait(&self, wait: Duration) {
    let mut m = self.inner.lock().unwrap();
    m.rate_limit_waits += 1;
    m.total_rate_limit_wait += wait;
  }

  pub fn record_circuit_trip(&self) {
    let mut m = self.inner.lock().unwrap();
    m.circuit_trips += 1;
  }

  pub fn record_llm_error(&self) {
    let mut m = self.inner.lock().unwrap();
    m.llm_errors += 1;
  }

  pub fn record_tool_call(&self, time: Duration) {
    let mut m = self.inner.lock().unwrap();
    m.tool_call_count += 1;
    m.total_tool_time += time;
  }

  pub fn record_tokens(&self, input: u64, output: u64) {
    let mut m = self.inner.lock().unwrap();
    m.total_input_tokens += input;
    m.total_output_tokens += output;
  }

  pub fn snapshot(&self) -> CallMetrics {
    self.inner.lock().unwrap().clone()
  }

  pub fn reset(&self) {
    let mut m = self.inner.lock().unwrap();
    *m = CallMetrics::default();
  }
}

impl Default for MetricsCollector {
  fn default() -> Self {
    Self::new()
  }
}