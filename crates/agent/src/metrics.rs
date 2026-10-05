//! Call and token metrics collected while running an agent.

use std::sync::Mutex;
use std::time::Duration;

/// Aggregated counters and timings for an agent run.
#[derive(Debug, Clone, Default)]
pub struct CallMetrics {
    /// Number of LLM calls made.
    pub llm_call_count: u64,
    /// Total wall-clock time spent in LLM calls.
    pub total_wall_time: Duration,
    /// Time spent inside the ReAct engine.
    pub total_engine_time: Duration,
    /// Time spent waiting on retries and backoff.
    pub total_resilience_time: Duration,
    /// Number of rate-limit waits.
    pub rate_limit_waits: u64,
    /// Total time spent waiting on rate limits.
    pub total_rate_limit_wait: Duration,
    /// Number of circuit-breaker trips.
    pub circuit_trips: u64,
    /// Number of failed LLM calls.
    pub llm_errors: u64,
    /// Number of tool invocations.
    pub tool_invocation_count: u64,
    /// Total time spent running tools.
    pub total_tool_time: Duration,
    /// Prompt tokens consumed.
    pub total_input_tokens: u64,
    /// Completion tokens produced.
    pub total_output_tokens: u64,
}

/// Thread-safe collector that accumulates [`CallMetrics`].
#[derive(Debug)]
pub struct MetricsCollector {
    inner: Mutex<CallMetrics>,
}

impl MetricsCollector {
    /// Create an empty collector.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(CallMetrics::default()),
        }
    }

    /// Record one LLM call with its timings and token counts.
    pub fn record_call(
        &self,
        wall_time: Duration,
        engine_time: Duration,
        resilience_time: Duration,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        let mut m = self.inner.lock().unwrap();
        m.llm_call_count += 1;
        m.total_wall_time += wall_time;
        m.total_engine_time += engine_time;
        m.total_resilience_time += resilience_time;
        m.total_input_tokens += input_tokens;
        m.total_output_tokens += output_tokens;
    }

    /// Record one rate-limit wait.
    pub fn record_rate_limit_wait(&self, wait: Duration) {
        let mut m = self.inner.lock().unwrap();
        m.rate_limit_waits += 1;
        m.total_rate_limit_wait += wait;
    }

    /// Record one circuit-breaker trip.
    pub fn record_circuit_trip(&self) {
        let mut m = self.inner.lock().unwrap();
        m.circuit_trips += 1;
    }

    /// Record one failed LLM call.
    pub fn record_llm_error(&self) {
        let mut m = self.inner.lock().unwrap();
        m.llm_errors += 1;
    }

    /// Record one tool invocation and its duration.
    pub fn record_tool_call(&self, time: Duration) {
        let mut m = self.inner.lock().unwrap();
        m.tool_invocation_count += 1;
        m.total_tool_time += time;
    }

    /// Record several tool invocations and their combined duration.
    pub fn record_tool_calls(&self, count: u64, total_time: Duration) {
        let mut m = self.inner.lock().unwrap();
        m.tool_invocation_count += count;
        m.total_tool_time += total_time;
    }

    /// Add token counts without recording a call.
    pub fn record_tokens(&self, input: u64, output: u64) {
        let mut m = self.inner.lock().unwrap();
        m.total_input_tokens += input;
        m.total_output_tokens += output;
    }

    /// Copy the current metrics.
    pub fn snapshot(&self) -> CallMetrics {
        self.inner.lock().unwrap().clone()
    }

    /// Zero every metric.
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
