//! Token budgets and lightweight telemetry events for a ReAct run.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A structured telemetry event emitted during a run.
pub enum TelemetryEvent {
    /// An LLM call completed.
    LlmCall {
        /// Model that served the call.
        model: String,
        /// Total tokens billed.
        tokens: u32,
    },
    /// A tool call completed.
    ToolCall {
        /// Tool that was called.
        tool: String,
        /// Wall-clock duration in milliseconds.
        duration_ms: u64,
    },
    /// An error was recorded.
    Error {
        /// Error message.
        error: String,
    },
    /// An arbitrary checkpoint value.
    Checkpoint(serde_json::Value),
    /// A tool invocation with its input and output.
    ToolInvocation {
        /// Tool that was invoked.
        tool: String,
        /// Arguments passed to the tool.
        input: serde_json::Value,
        /// Value returned by the tool.
        output: serde_json::Value,
    },
    /// The run produced a final answer.
    FinalAnswer {
        /// The final answer text.
        answer: String,
    },
}

#[derive(Debug, Clone)]
/// Emits telemetry events through the log facade.
pub struct Telemetry {
    enabled: bool,
}

impl Telemetry {
    /// Create enabled telemetry.
    pub fn new() -> Self {
        Self { enabled: true }
    }

    /// Emit `event`, if telemetry is enabled.
    pub fn emit(&self, event: &TelemetryEvent) {
        if self.enabled {
            match event {
                TelemetryEvent::LlmCall { model, tokens } => {
                    log::debug!("LLM call: model={}, tokens={}", model, tokens);
                }
                TelemetryEvent::ToolCall { tool, duration_ms } => {
                    log::debug!("Tool call: tool={}, duration_ms={}", tool, duration_ms);
                }
                TelemetryEvent::Error { error } => {
                    log::error!("Telemetry error: {}", error);
                }
                TelemetryEvent::Checkpoint(data) => {
                    log::debug!("Checkpoint: {}", data);
                }
                TelemetryEvent::ToolInvocation {
                    tool,
                    input,
                    output,
                } => {
                    log::debug!("Tool: {} input={} output={}", tool, input, output);
                }
                TelemetryEvent::FinalAnswer { answer } => {
                    log::debug!("Final answer: {}", answer);
                }
            }
        }
    }
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Limits and thresholds for a token budget.
pub struct TokenBudgetConfig {
    /// Token ceiling for a single request.
    pub max_request_tokens: u32,
    /// Percentage at which a warning is raised.
    pub warning_threshold_percent: u8,
    /// Token ceiling for retained history.
    pub max_history_tokens: u32,
    /// Whether to compact automatically when nearing the budget.
    pub auto_compact: bool,
}

impl Default for TokenBudgetConfig {
    fn default() -> Self {
        Self {
            max_request_tokens: 128_000,
            warning_threshold_percent: 80,
            max_history_tokens: 64_000,
            auto_compact: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// Token counts for one or more requests.
pub struct TokenUsage {
    /// Tokens in the prompt.
    pub prompt_tokens: u32,
    /// Tokens in the completion.
    pub completion_tokens: u32,
    /// Prompt plus completion tokens.
    pub total_tokens: u32,
    /// Prompt tokens the provider served from its own prompt cache.
    pub cached_tokens: u32,
}

impl TokenUsage {
    /// State how many prompt tokens came from the provider's cache.
    ///
    /// Kept off TokenUsage::new so every existing caller keeps building the
    /// value it built before a cache was ever reported.
    pub fn with_cached(mut self, cached: u32) -> Self {
        self.cached_tokens = cached;
        self
    }

    /// Build usage from prompt and completion counts, summing the total.
    pub fn new(prompt: u32, completion: u32) -> Self {
        Self {
            prompt_tokens: prompt,
            completion_tokens: completion,
            cached_tokens: 0,
            total_tokens: prompt + completion,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// How close usage is to the configured budget.
pub enum BudgetStatus {
    /// Within budget.
    Normal,
    /// At or above the warning threshold.
    Warning,
    /// Over the request ceiling.
    Exceeded,
    /// At or above the full budget.
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A snapshot of usage against a [`TokenBudgetConfig`].
pub struct TokenBudgetReport {
    /// Usage being reported.
    pub usage: TokenUsage,
    /// Budget the usage was measured against.
    pub config: TokenBudgetConfig,
    /// Whether usage is normal, warning, exceeded, or critical.
    pub status: BudgetStatus,
    /// Usage as a percentage of the request ceiling.
    pub usage_percent: f32,
    /// Tokens left before the request ceiling.
    pub remaining_tokens: u32,
}

impl TokenBudgetReport {
    /// Classify `usage` against `config`.
    pub fn new(usage: TokenUsage, config: &TokenBudgetConfig) -> Self {
        let usage_percent = if config.max_request_tokens > 0 {
            (usage.total_tokens as f32 / config.max_request_tokens as f32) * 100.0
        } else {
            0.0
        };

        let remaining = config.max_request_tokens.saturating_sub(usage.total_tokens);

        let status = if usage_percent >= 100.0 {
            BudgetStatus::Critical
        } else if usage_percent >= config.warning_threshold_percent as f32 {
            BudgetStatus::Warning
        } else if usage.total_tokens > config.max_request_tokens {
            BudgetStatus::Exceeded
        } else {
            BudgetStatus::Normal
        };

        Self {
            usage,
            config: config.clone(),
            status,
            usage_percent,
            remaining_tokens: remaining,
        }
    }
}

#[derive(Debug)]
/// Tracks token usage and budget status across a session.
pub struct TokenCounter {
    config: TokenBudgetConfig,
    current_usage: AtomicTokenUsage,
    total_requests: AtomicU64,
    total_prompt_tokens: AtomicU64,
    total_completion_tokens: AtomicU64,
    total_cached_tokens: AtomicU64,
}

#[derive(Debug)]
/// Lock-free token counters.
pub struct AtomicTokenUsage {
    /// Atomic prompt token count.
    pub prompt_tokens: AtomicU32,
    /// Atomic completion token count.
    pub completion_tokens: AtomicU32,
    /// Atomic total token count.
    pub total_tokens: AtomicU32,
    /// Atomic cached prompt token count.
    pub cached_tokens: AtomicU32,
}

impl Default for AtomicTokenUsage {
    fn default() -> Self {
        Self::new()
    }
}

impl AtomicTokenUsage {
    /// Create zeroed counters.
    pub fn new() -> Self {
        Self {
            prompt_tokens: AtomicU32::new(0),
            completion_tokens: AtomicU32::new(0),
            total_tokens: AtomicU32::new(0),
            cached_tokens: AtomicU32::new(0),
        }
    }

    /// Replace all three counters from `usage`.
    pub fn set(&self, usage: TokenUsage) {
        self.prompt_tokens
            .store(usage.prompt_tokens, Ordering::Relaxed);
        self.completion_tokens
            .store(usage.completion_tokens, Ordering::Relaxed);
        self.total_tokens
            .store(usage.total_tokens, Ordering::Relaxed);
        self.cached_tokens
            .store(usage.cached_tokens, Ordering::Relaxed);
    }

    /// Read the counters as a [`TokenUsage`].
    pub fn get(&self) -> TokenUsage {
        TokenUsage {
            prompt_tokens: self.prompt_tokens.load(Ordering::Relaxed),
            completion_tokens: self.completion_tokens.load(Ordering::Relaxed),
            total_tokens: self.total_tokens.load(Ordering::Relaxed),
            cached_tokens: self.cached_tokens.load(Ordering::Relaxed),
        }
    }
}

impl TokenCounter {
    /// Create a counter with `config`.
    pub fn new(config: TokenBudgetConfig) -> Self {
        Self {
            config,
            current_usage: AtomicTokenUsage::new(),
            total_requests: AtomicU64::new(0),
            total_prompt_tokens: AtomicU64::new(0),
            total_completion_tokens: AtomicU64::new(0),
            total_cached_tokens: AtomicU64::new(0),
        }
    }

    /// Create a counter with the default budget.
    pub fn with_default() -> Self {
        Self::new(TokenBudgetConfig::default())
    }

    /// Record exact usage from a response and count the request.
    pub fn update_from_response(&self, usage: TokenUsage) {
        self.total_prompt_tokens
            .fetch_add(u64::from(usage.prompt_tokens), Ordering::Relaxed);
        self.total_completion_tokens
            .fetch_add(u64::from(usage.completion_tokens), Ordering::Relaxed);
        self.total_cached_tokens
            .fetch_add(u64::from(usage.cached_tokens), Ordering::Relaxed);
        self.current_usage.set(usage);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Lifetime prompt and completion totals across every recorded request.
    ///
    /// Unlike [`TokenCounter::usage`], which reports only the current
    /// request, these sums accumulate for as long as the counter exists.
    /// Lifetime count of prompt tokens the provider served from its cache.
    ///
    /// Zero for a provider that never reports the breakdown, which is why the
    /// caller states a cache hit as a dash rather than as 0%.
    pub fn lifetime_cached_tokens(&self) -> u64 {
        self.total_cached_tokens.load(Ordering::Relaxed)
    }

    /// Lifetime prompt and completion totals across every recorded request.
    ///
    /// Unlike [TokenCounter::usage], which reports only the current request,
    /// these sums accumulate for as long as the counter exists.
    pub fn lifetime_totals(&self) -> (u64, u64) {
        (
            self.total_prompt_tokens.load(Ordering::Relaxed),
            self.total_completion_tokens.load(Ordering::Relaxed),
        )
    }

    /// Classify current usage against the budget.
    pub fn budget_report(&self) -> TokenBudgetReport {
        TokenBudgetReport::new(self.current_usage.get(), &self.config)
    }

    /// Whether auto-compaction is due under the current status.
    pub fn needs_compaction(&self) -> bool {
        self.config.auto_compact
            && matches!(
                self.budget_report().status,
                BudgetStatus::Warning | BudgetStatus::Exceeded | BudgetStatus::Critical
            )
    }

    /// Number of requests recorded.
    pub fn total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }

    /// Usage of the current request.
    pub fn current_usage(&self) -> TokenUsage {
        self.current_usage.get()
    }

    /// Alias for [`TokenCounter::current_usage`].
    pub fn usage(&self) -> TokenUsage {
        self.current_usage.get()
    }

    /// Alias for [`TokenCounter::budget_report`].
    pub fn report(&self) -> TokenBudgetReport {
        self.budget_report()
    }

    /// The configured budget.
    pub fn config(&self) -> &TokenBudgetConfig {
        &self.config
    }
}

impl Default for TokenCounter {
    fn default() -> Self {
        Self::with_default()
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn a_provider_cache_breakdown_is_counted_and_absent_means_unknown() {
        let counter = TokenCounter::with_default();
        assert_eq!(counter.lifetime_cached_tokens(), 0, "nothing reported yet");
        counter.update_from_response(TokenUsage::new(120, 45).with_cached(96));
        assert_eq!(counter.lifetime_cached_tokens(), 96);
        // A provider that reports no breakdown leaves the count where it was: the
        // caller has to be able to tell "nothing was cached" from "not reported",
        // and the foot states the second as a dash rather than as 0%.
        counter.update_from_response(TokenUsage::new(10, 2));
        assert_eq!(counter.lifetime_cached_tokens(), 96);
        assert_eq!(counter.lifetime_totals(), (130, 47));
    }
}
