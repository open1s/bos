use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TelemetryEvent {
    LlmCall {
        model: String,
        tokens: u32,
    },
    ToolCall {
        tool: String,
        duration_ms: u64,
    },
    Error {
        error: String,
    },
    Checkpoint(serde_json::Value),
    ToolInvocation {
        tool: String,
        input: serde_json::Value,
        output: serde_json::Value,
    },
    FinalAnswer {
        answer: String,
    },
}

#[derive(Debug, Clone)]
pub struct Telemetry {
    enabled: bool,
}

impl Telemetry {
    pub fn new() -> Self {
        Self { enabled: true }
    }

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
pub struct TokenBudgetConfig {
    pub max_request_tokens: u32,
    pub warning_threshold_percent: u8,
    pub max_history_tokens: u32,
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
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl TokenUsage {
    pub fn new(prompt: u32, completion: u32) -> Self {
        Self {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
        }
    }

    pub fn estimate_from_text(text: &str) -> u32 {
        (text.len() / 4) as u32
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BudgetStatus {
    Normal,
    Warning,
    Exceeded,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudgetReport {
    pub usage: TokenUsage,
    pub config: TokenBudgetConfig,
    pub status: BudgetStatus,
    pub usage_percent: f32,
    pub remaining_tokens: u32,
}

impl TokenBudgetReport {
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
pub struct TokenCounter {
    config: TokenBudgetConfig,
    current_usage: AtomicTokenUsage,
    total_requests: AtomicU64,
    session_start_tokens: u64,
}

#[derive(Debug)]
pub struct AtomicTokenUsage {
    pub prompt_tokens: AtomicU32,
    pub completion_tokens: AtomicU32,
    pub total_tokens: AtomicU32,
}

impl AtomicTokenUsage {
    pub fn new() -> Self {
        Self {
            prompt_tokens: AtomicU32::new(0),
            completion_tokens: AtomicU32::new(0),
            total_tokens: AtomicU32::new(0),
        }
    }

    pub fn set(&self, usage: TokenUsage) {
        self.prompt_tokens
            .store(usage.prompt_tokens, Ordering::Relaxed);
        self.completion_tokens
            .store(usage.completion_tokens, Ordering::Relaxed);
        self.total_tokens
            .store(usage.total_tokens, Ordering::Relaxed);
    }

    pub fn get(&self) -> TokenUsage {
        TokenUsage {
            prompt_tokens: self.prompt_tokens.load(Ordering::Relaxed),
            completion_tokens: self.completion_tokens.load(Ordering::Relaxed),
            total_tokens: self.total_tokens.load(Ordering::Relaxed),
        }
    }
}

impl TokenCounter {
    pub fn new(config: TokenBudgetConfig) -> Self {
        Self {
            config,
            current_usage: AtomicTokenUsage::new(),
            total_requests: AtomicU64::new(0),
            session_start_tokens: 0,
        }
    }

    pub fn with_default() -> Self {
        Self::new(TokenBudgetConfig::default())
    }

    pub fn update_from_response(&self, usage: TokenUsage) {
        self.current_usage.set(usage);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn estimate_and_update(&self, prompt_text: &str) {
        let estimated = TokenUsage::estimate_from_text(prompt_text);
        let current = self.current_usage.get();
        let new_usage =
            TokenUsage::new(current.prompt_tokens + estimated, current.completion_tokens);
        self.current_usage.set(new_usage);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn budget_report(&self) -> TokenBudgetReport {
        TokenBudgetReport::new(self.current_usage.get(), &self.config)
    }

    pub fn needs_compaction(&self) -> bool {
        self.config.auto_compact
            && matches!(
                self.budget_report().status,
                BudgetStatus::Warning | BudgetStatus::Exceeded | BudgetStatus::Critical
            )
    }

    pub fn reset_session(&mut self) {
        self.session_start_tokens += self.current_usage.get().total_tokens as u64;
        self.current_usage.set(TokenUsage::default());
    }

    pub fn session_total_tokens(&self) -> u64 {
        self.session_start_tokens + self.current_usage.get().total_tokens as u64
    }

    pub fn total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }

    pub fn current_usage(&self) -> TokenUsage {
        self.current_usage.get()
    }

    pub fn usage(&self) -> TokenUsage {
        self.current_usage.get()
    }

    pub fn report(&self) -> TokenBudgetReport {
        self.budget_report()
    }

    pub fn config(&self) -> &TokenBudgetConfig {
        &self.config
    }

    pub fn set_max_tokens(&mut self, max: u32) {
        self.config.max_request_tokens = max;
    }

    pub fn set_warning_threshold(&mut self, percent: u8) {
        self.config.warning_threshold_percent = percent;
    }

    pub fn set_auto_compact(&mut self, enabled: bool) {
        self.config.auto_compact = enabled;
    }
}

impl Default for TokenCounter {
    fn default() -> Self {
        Self::with_default()
    }
}
