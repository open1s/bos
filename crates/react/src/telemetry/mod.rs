use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TelemetryEvent {
    LlmCall { model: String, tokens: u32 },
    ToolCall { tool: String, duration_ms: u64 },
    Error { error: String },
    Checkpoint(serde_json::Value),
    ToolInvocation { tool: String, input: serde_json::Value, output: serde_json::Value },
    FinalAnswer { answer: String },
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
                TelemetryEvent::ToolInvocation { tool, input, output } => {
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

#[derive(Debug, Clone)]
pub struct TokenCounter {
    config: TokenBudgetConfig,
    current_usage: TokenUsage,
    total_requests: u64,
    session_start_tokens: u64,
}

impl TokenCounter {
    pub fn new(config: TokenBudgetConfig) -> Self {
        Self {
            config,
            current_usage: TokenUsage::default(),
            total_requests: 0,
            session_start_tokens: 0,
        }
    }

    pub fn with_default() -> Self {
        Self::new(TokenBudgetConfig::default())
    }

    pub fn update_from_response(&mut self, usage: TokenUsage) {
        self.current_usage = usage;
        self.total_requests += 1;
    }

    pub fn estimate_and_update(&mut self, prompt_text: &str) {
        let estimated = TokenUsage::estimate_from_text(prompt_text);
        self.current_usage = TokenUsage::new(estimated, 0);
        self.total_requests += 1;
    }

    pub fn budget_report(&self) -> TokenBudgetReport {
        TokenBudgetReport::new(self.current_usage.clone(), &self.config)
    }

    pub fn needs_compaction(&self) -> bool {
        self.config.auto_compact
            && matches!(
                self.budget_report().status,
                BudgetStatus::Warning | BudgetStatus::Exceeded | BudgetStatus::Critical
            )
    }

    pub fn reset_session(&mut self) {
        self.session_start_tokens += self.current_usage.total_tokens as u64;
        self.current_usage = TokenUsage::default();
    }

    pub fn session_total_tokens(&self) -> u64 {
        self.session_start_tokens + self.current_usage.total_tokens as u64
    }

    pub fn total_requests(&self) -> u64 {
        self.total_requests
    }

    pub fn current_usage(&self) -> &TokenUsage {
        &self.current_usage
    }

    pub fn usage(&self) -> TokenUsage {
        self.current_usage.clone()
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