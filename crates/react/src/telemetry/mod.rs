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
}

impl TokenUsage {
    /// Build usage from prompt and completion counts, summing the total.
    pub fn new(prompt: u32, completion: u32) -> Self {
        Self {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
        }
    }

    /// Roughly estimate tokens as one quarter of the byte length.
    pub fn estimate_from_text(text: &str) -> u32 {
        (text.len() / 4) as u32
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
    session_start_tokens: u64,
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
    }

    /// Read the counters as a [`TokenUsage`].
    pub fn get(&self) -> TokenUsage {
        TokenUsage {
            prompt_tokens: self.prompt_tokens.load(Ordering::Relaxed),
            completion_tokens: self.completion_tokens.load(Ordering::Relaxed),
            total_tokens: self.total_tokens.load(Ordering::Relaxed),
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
            session_start_tokens: 0,
        }
    }

    /// Create a counter with the default budget.
    pub fn with_default() -> Self {
        Self::new(TokenBudgetConfig::default())
    }

    /// Record exact usage from a response and count the request.
    pub fn update_from_response(&self, usage: TokenUsage) {
        self.current_usage.set(usage);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Estimate prompt usage from `prompt_text` and count the request.
    pub fn estimate_and_update(&self, prompt_text: &str) {
        let estimated = TokenUsage::estimate_from_text(prompt_text);
        let current = self.current_usage.get();
        let new_usage =
            TokenUsage::new(current.prompt_tokens + estimated, current.completion_tokens);
        self.current_usage.set(new_usage);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
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

    /// Roll current usage into the session total and reset it.
    pub fn reset_session(&mut self) {
        self.session_start_tokens += self.current_usage.get().total_tokens as u64;
        self.current_usage.set(TokenUsage::default());
    }

    /// Total tokens seen since the session began.
    pub fn session_total_tokens(&self) -> u64 {
        self.session_start_tokens + self.current_usage.get().total_tokens as u64
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

    /// Set the request token ceiling.
    pub fn set_max_tokens(&mut self, max: u32) {
        self.config.max_request_tokens = max;
    }

    /// Set the warning threshold percentage.
    pub fn set_warning_threshold(&mut self, percent: u8) {
        self.config.warning_threshold_percent = percent;
    }

    /// Enable or disable auto-compaction.
    pub fn set_auto_compact(&mut self, enabled: bool) {
        self.config.auto_compact = enabled;
    }
}

impl Default for TokenCounter {
    fn default() -> Self {
        Self::with_default()
    }
}
