//! Token budget tracking for the ReAct engine.
//! Provides token counting and budget management capabilities.

use serde::{Deserialize, Serialize};

/// Configuration for token budget limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudgetConfig {
    /// Maximum tokens allowed in a single request (default: 128k)
    pub max_request_tokens: u32,
    /// Warning threshold percentage (default: 80%)
    pub warning_threshold_percent: u8,
    /// Maximum conversation history tokens (default: 64k)
    pub max_history_tokens: u32,
    /// Enable auto-compaction when limit reached
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

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Prompt tokens used
    pub prompt_tokens: u32,
    /// Completion tokens used
    pub completion_tokens: u32,
    /// Total tokens used (prompt + completion)
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

    /// Estimate tokens from text (rough approximation: ~4 chars per token)
    pub fn estimate_from_text(text: &str) -> u32 {
        (text.len() / 4) as u32
    }
}

/// Token budget status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BudgetStatus {
    /// Under warning threshold
    Normal,
    /// Approaching limit (over warning threshold)
    Warning,
    /// Over budget limit
    Exceeded,
    /// Requires immediate action
    Critical,
}

/// Token budget report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudgetReport {
    /// Current usage
    pub usage: TokenUsage,
    /// Budget configuration
    pub config: TokenBudgetConfig,
    /// Current status
    pub status: BudgetStatus,
    /// Usage percentage of max request tokens
    pub usage_percent: f32,
    /// Remaining tokens available
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

/// Token counter for tracking and managing token budgets
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

    /// Update token usage from an LLM response
    pub fn update_from_response(&mut self, usage: TokenUsage) {
        self.current_usage = usage;
        self.total_requests += 1;
    }

    /// Update tokens from estimated text input
    pub fn estimate_and_update(&mut self, prompt_text: &str) {
        let estimated = TokenUsage::estimate_from_text(prompt_text);
        self.current_usage = TokenUsage::new(estimated, 0);
        self.total_requests += 1;
    }

    /// Get current budget report
    pub fn budget_report(&self) -> TokenBudgetReport {
        TokenBudgetReport::new(self.current_usage.clone(), &self.config)
    }

    /// Check if auto-compaction is needed
    pub fn needs_compaction(&self) -> bool {
        self.config.auto_compact
            && matches!(
                self.budget_report().status,
                BudgetStatus::Warning | BudgetStatus::Exceeded | BudgetStatus::Critical
            )
    }

    /// Reset current session usage
    pub fn reset_session(&mut self) {
        self.session_start_tokens += self.current_usage.total_tokens as u64;
        self.current_usage = TokenUsage::default();
    }

    /// Get total tokens used in session (all requests)
    pub fn session_total_tokens(&self) -> u64 {
        self.session_start_tokens + self.current_usage.total_tokens as u64
    }

    /// Get number of requests made
    pub fn total_requests(&self) -> u64 {
        self.total_requests
    }

    /// Get current usage
    pub fn current_usage(&self) -> &TokenUsage {
        &self.current_usage
    }

    pub fn usage(&self) -> TokenUsage {
        self.current_usage.clone()
    }

    pub fn report(&self) -> TokenBudgetReport {
        self.budget_report()
    }

    /// Get config reference
    pub fn config(&self) -> &TokenBudgetConfig {
        &self.config
    }

    /// Update max request tokens
    pub fn set_max_tokens(&mut self, max: u32) {
        self.config.max_request_tokens = max;
    }

    /// Update warning threshold
    pub fn set_warning_threshold(&mut self, percent: u8) {
        self.config.warning_threshold_percent = percent;
    }

    /// Toggle auto-compaction
    pub fn set_auto_compact(&mut self, enabled: bool) {
        self.config.auto_compact = enabled;
    }
}

impl Default for TokenCounter {
    fn default() -> Self {
        Self::with_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_usage() {
        let usage = TokenUsage::new(1000, 500);
        assert_eq!(usage.total_tokens, 1500);
    }

    #[test]
    fn test_estimate_from_text() {
        let text = "This is a test string with twenty eight characters.";
        let tokens = TokenUsage::estimate_from_text(text);
        assert!(tokens > 0);
    }

    #[test]
    fn test_budget_report_normal() {
        let config = TokenBudgetConfig::default();
        let usage = TokenUsage::new(1000, 500); // 1500 total
        let report = TokenBudgetReport::new(usage, &config);

        assert!(matches!(report.status, BudgetStatus::Normal));
        assert!(report.usage_percent < 80.0);
    }

    #[test]
    fn test_budget_report_warning() {
        let config = TokenBudgetConfig {
            max_request_tokens: 1000,
            warning_threshold_percent: 20,
            ..Default::default()
        };
        let usage = TokenUsage::new(800, 0);
        let report = TokenBudgetReport::new(usage, &config);

        assert!(matches!(
            report.status,
            BudgetStatus::Warning | BudgetStatus::Exceeded
        ));
    }

    #[test]
    fn test_token_counter() {
        let mut counter = TokenCounter::with_default();

        counter.estimate_and_update("Hello world");

        let report = counter.budget_report();
        assert!(report.usage.total_tokens > 0);
    }

    #[test]
    fn test_needs_compaction_disabled() {
        let config = TokenBudgetConfig {
            auto_compact: false,
            ..Default::default()
        };
        let counter = TokenCounter::new(config);

        assert!(!counter.needs_compaction());
    }

    #[test]
    fn test_needs_compaction_enabled() {
        let config = TokenBudgetConfig {
            auto_compact: true,
            max_request_tokens: 100,
            ..Default::default()
        };
        let mut counter = TokenCounter::new(config);

        // Set usage over threshold
        counter.update_from_response(TokenUsage::new(90, 20));

        assert!(counter.needs_compaction());
    }

    #[test]
    fn test_session_tracking() {
        let mut counter = TokenCounter::with_default();

        counter.estimate_and_update("Request 1");
        assert_eq!(counter.total_requests(), 1);

        counter.estimate_and_update("Request 2");
        assert_eq!(counter.total_requests(), 2);
    }
}
