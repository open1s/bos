//! Token budgets and lightweight telemetry events for a ReAct run.

mod composition;

pub use composition::RequestComposition;

use crate::llm::ActionIntent;
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
        /// The turn this answer closes.
        turn: u64,
        /// The final answer text.
        answer: String,
    },
    /// One on-demand discovery changed the live tool array.
    ///
    /// Counts only: the query text, tool arguments and results are not
    /// recorded here, which is the privacy default for this evaluation loop.
    ToolDiscovery {
        /// The turn this discovery belongs to.
        turn: u64,
        /// Definitions the result offered.
        candidates: usize,
        /// Definitions added to the next request.
        revealed: usize,
        /// Definitions refused by the schema budget.
        rejected: usize,
        /// Cumulative schema bytes spent this turn.
        spent_bytes: usize,
    },
    /// One request priced before it is sent.
    ///
    /// The parts are the ones a provider caches and bills: system prompt, tool
    /// definitions, skill catalogue, history and input. Only counts are
    /// recorded - no message text, tool argument or credential - so the event
    /// can be logged as it stands.
    RequestComposition {
        /// The turn this request belongs to.
        turn: u64,
        /// The per-component byte counts.
        composition: RequestComposition,
        /// Schema bytes this turn has spent, initial definitions included.
        schema_spent_bytes: usize,
        /// The turn's schema budget; zero when the budget is off.
        schema_limit_bytes: usize,
    },
    /// One turn closed, with the action-versus-talk facts.
    ///
    /// The counts answer the question a prompt review cannot: did an ACTION
    /// turn act before it talked, and did an EXPLANATION turn pay for tools it
    /// did not need? No text, arguments or prompts are recorded.
    TurnOutcome {
        /// The turn that closed.
        turn: u64,
        /// The host's classification of the request.
        intent: ActionIntent,
        /// Callable definitions the request carried.
        tools_available: usize,
        /// Tool calls the turn ran; zero is a talk-only turn.
        tool_calls: usize,
        /// The step (1-based request) that ran the first tool call.
        first_tool_step: Option<usize>,
        /// Requests the turn spent.
        steps: usize,
        /// Whether the engine had to ask an ACTION turn to act.
        action_nudged: bool,
        /// Bytes of the final answer.
        answer_bytes: usize,
    },
    /// One tool call selected from the model's live array.
    ///
    /// `discovered` distinguishes a tool that entered through find_tool from
    /// one that was advertised from the start, which is the selection signal
    /// the effectiveness loop correlates with the turn's discovery events.
    ToolSelection {
        /// The turn this selection belongs to.
        turn: u64,
        /// The tool the model called.
        tool: String,
        /// Whether the schema was revealed by discovery this turn.
        discovered: bool,
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
                TelemetryEvent::FinalAnswer { turn, answer } => {
                    log::debug!("Final answer: turn={}, {}", turn, answer);
                }
                TelemetryEvent::ToolDiscovery {
                    turn,
                    candidates,
                    revealed,
                    rejected,
                    spent_bytes,
                } => {
                    log::debug!(
                        "Tool discovery: turn={}, candidates={}, revealed={}, rejected={}, spent={} bytes",
                        turn,
                        candidates,
                        revealed,
                        rejected,
                        spent_bytes
                    );
                }
                TelemetryEvent::RequestComposition {
                    turn,
                    composition,
                    schema_spent_bytes,
                    schema_limit_bytes,
                } => {
                    // One line per step: a reader can attribute a captured
                    // request to a part instead of guessing from the total.
                    log::info!(
                        "request: turn={} total={} bytes system={} tools={} ({} defs, largest {}:{} bytes) skills={} history={} input={} schema_budget={}/{}",
                        turn,
                        composition.total_bytes,
                        composition.system_bytes,
                        composition.tools_bytes,
                        composition.tool_count,
                        composition
                            .largest_tool
                            .as_deref()
                            .unwrap_or("-"),
                        composition.largest_tool_bytes,
                        composition.skills_bytes,
                        composition.history_bytes,
                        composition.input_bytes,
                        schema_spent_bytes,
                        schema_limit_bytes
                    );
                }
                TelemetryEvent::TurnOutcome {
                    turn,
                    intent,
                    tools_available,
                    tool_calls,
                    first_tool_step,
                    steps,
                    action_nudged,
                    answer_bytes,
                } => {
                    log::info!(
                        "turn outcome: turn={} intent={} tools={} tool_calls={} first_tool_step={} steps={} nudged={} answer_bytes={}",
                        turn,
                        intent.as_str(),
                        tools_available,
                        tool_calls,
                        first_tool_step
                            .map(|step| step.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        steps,
                        action_nudged,
                        answer_bytes
                    );
                }
                TelemetryEvent::ToolSelection {
                    turn,
                    tool,
                    discovered,
                } => {
                    log::debug!(
                        "Tool selection: turn={}, tool={}, discovered={}",
                        turn,
                        tool,
                        discovered
                    );
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

/// Aggregate figures for the tool-selection lifecycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolEffectivenessReport {
    /// Distinct turns observed.
    pub turns: usize,
    /// Turns that reached a final answer.
    pub completed_turns: usize,
    /// Discovery calls.
    pub discoveries: usize,
    /// Candidate definitions offered across discoveries.
    pub candidates: usize,
    /// Definitions added to a request.
    pub revealed: usize,
    /// Definitions refused by the schema budget.
    pub rejected: usize,
    /// Tool calls made.
    pub selections: usize,
    /// Tool calls whose tool was revealed by discovery this turn.
    pub discovered_selections: usize,
    /// Cumulative schema bytes spent.
    pub schema_bytes: usize,
}

impl ToolEffectivenessReport {
    /// Share of selections that used a discovered tool. Zero when nothing was
    /// called.
    pub fn discovery_use_rate(&self) -> f64 {
        if self.selections == 0 {
            0.0
        } else {
            self.discovered_selections as f64 / self.selections as f64
        }
    }

    /// Share of offered definitions that were refused by the budget.
    pub fn rejection_rate(&self) -> f64 {
        let offered = self.revealed + self.rejected;
        if offered == 0 {
            0.0
        } else {
            self.rejected as f64 / offered as f64
        }
    }

    /// Mean candidates per discovery.
    pub fn mean_candidates(&self) -> f64 {
        if self.discoveries == 0 {
            0.0
        } else {
            self.candidates as f64 / self.discoveries as f64
        }
    }
}

/// Accumulates correlated tool events for one host.
///
/// The accumulator is fed the same `TelemetryEvent` stream the engine emits, so
/// a host can build an offline report without parsing logs.
#[derive(Debug, Clone, Default)]
pub struct ToolEffectiveness {
    turns: std::collections::BTreeSet<u64>,
    completed_turns: usize,
    discoveries: usize,
    candidates: usize,
    revealed: usize,
    rejected: usize,
    selections: usize,
    discovered_selections: usize,
    schema_bytes: usize,
}

impl ToolEffectiveness {
    /// Record one event.
    pub fn record(&mut self, event: &TelemetryEvent) {
        match event {
            TelemetryEvent::ToolDiscovery {
                turn,
                candidates,
                revealed,
                rejected,
                spent_bytes,
            } => {
                self.turns.insert(*turn);
                self.discoveries += 1;
                self.candidates += candidates;
                self.revealed += revealed;
                self.rejected += rejected;
                self.schema_bytes += spent_bytes;
            }
            TelemetryEvent::ToolSelection {
                turn, discovered, ..
            } => {
                self.turns.insert(*turn);
                self.selections += 1;
                if *discovered {
                    self.discovered_selections += 1;
                }
            }
            TelemetryEvent::FinalAnswer { turn, .. } => {
                self.turns.insert(*turn);
                self.completed_turns += 1;
            }
            _ => {}
        }
    }

    /// The current report.
    pub fn report(&self) -> ToolEffectivenessReport {
        ToolEffectivenessReport {
            turns: self.turns.len(),
            completed_turns: self.completed_turns,
            discoveries: self.discoveries,
            candidates: self.candidates,
            revealed: self.revealed,
            rejected: self.rejected,
            selections: self.selections,
            discovered_selections: self.discovered_selections,
            schema_bytes: self.schema_bytes,
        }
    }
}

#[cfg(test)]
mod effectiveness_tests {
    use super::*;

    #[test]
    fn discovery_selection_and_outcome_correlate_by_turn() {
        let mut effectiveness = ToolEffectiveness::default();
        effectiveness.record(&TelemetryEvent::ToolDiscovery {
            turn: 1,
            candidates: 3,
            revealed: 2,
            rejected: 1,
            spent_bytes: 1200,
        });
        effectiveness.record(&TelemetryEvent::ToolSelection {
            turn: 1,
            tool: "deploy".to_string(),
            discovered: true,
        });
        effectiveness.record(&TelemetryEvent::ToolSelection {
            turn: 1,
            tool: "bash".to_string(),
            discovered: false,
        });
        effectiveness.record(&TelemetryEvent::FinalAnswer {
            turn: 1,
            answer: "done".to_string(),
        });
        effectiveness.record(&TelemetryEvent::ToolDiscovery {
            turn: 2,
            candidates: 1,
            revealed: 1,
            rejected: 0,
            spent_bytes: 200,
        });
        effectiveness.record(&TelemetryEvent::FinalAnswer {
            turn: 2,
            answer: "done".to_string(),
        });

        let report = effectiveness.report();
        assert_eq!(report.turns, 2);
        assert_eq!(report.completed_turns, 2);
        assert_eq!(report.discoveries, 2);
        assert_eq!(report.revealed, 3);
        assert_eq!(report.rejected, 1);
        assert_eq!(report.selections, 2);
        assert_eq!(report.discovered_selections, 1);
        assert!((report.discovery_use_rate() - 0.5).abs() < f64::EPSILON);
        assert!((report.rejection_rate() - 0.25).abs() < f64::EPSILON);
        assert!((report.mean_candidates() - 2.0).abs() < f64::EPSILON);
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
            // Provider token counts are u32 inputs; a sum that overflows must
            // saturate rather than panic in a debug build.
            total_tokens: prompt.saturating_add(completion),
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
    fn an_overflowing_token_sum_saturates() {
        let usage = TokenUsage::new(u32::MAX, 10);
        assert_eq!(usage.total_tokens, u32::MAX);
    }

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
