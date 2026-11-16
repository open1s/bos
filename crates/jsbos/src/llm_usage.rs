use napi_derive::napi;

#[napi(object)]
/// Token details for the prompt portion of a call.
pub struct PromptTokensDetails {
  /// Audio tokens in the prompt.
  #[napi(js_name = "audioTokens")]
  pub audio_tokens: Option<u32>,
  /// Cached tokens in the prompt.
  #[napi(js_name = "cachedTokens")]
  pub cached_tokens: Option<u32>,
}

#[napi(object)]
/// Token usage reported by an LLM provider.
pub struct LlmUsage {
  /// Prompt tokens.
  #[napi(js_name = "promptTokens")]
  pub prompt_tokens: u32,
  /// Completion tokens.
  #[napi(js_name = "completionTokens")]
  pub completion_tokens: u32,
  /// Total tokens.
  #[napi(js_name = "totalTokens")]
  pub total_tokens: u32,
  /// Optional prompt token breakdown.
  #[napi(js_name = "promptTokensDetails")]
  pub prompt_tokens_details: Option<PromptTokensDetails>,
}

#[napi]
/// Status of a token budget.
pub enum BudgetStatus {
  /// Within budget.
  Normal,
  /// Nearing the budget.
  Warning,
  /// Over the budget.
  Exceeded,
  /// Critically over the budget.
  Critical,
}

#[napi(object)]
/// Token usage for a single agent run.
pub struct TokenUsage {
  /// Prompt tokens.
  #[napi(js_name = "promptTokens")]
  pub prompt_tokens: u32,
  /// Completion tokens.
  #[napi(js_name = "completionTokens")]
  pub completion_tokens: u32,
  /// Total tokens.
  #[napi(js_name = "totalTokens")]
  pub total_tokens: u32,
}

#[napi(object)]
/// A token budget report.
pub struct TokenBudgetReport {
  /// Token usage included in the report.
  pub usage: TokenUsage,
  /// Budget status.
  pub status: BudgetStatus,
  /// Fraction of the budget used.
  #[napi(js_name = "usagePercent")]
  pub usage_percent: f64,
  /// Tokens left in the budget.
  #[napi(js_name = "remainingTokens")]
  pub remaining_tokens: u32,
}
