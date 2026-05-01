use napi_derive::napi;
use react::token_counter::{
  BudgetStatus, TokenBudgetReport as InnerTokenBudgetReport, TokenUsage as InnerTokenUsage,
};

#[napi(object)]
pub struct PromptTokensDetails {
  #[napi(js_name = "audioTokens")]
  pub audio_tokens: Option<u32>,
  #[napi(js_name = "cachedTokens")]
  pub cached_tokens: Option<u32>,
}

#[napi(object)]
pub struct LlmUsage {
  #[napi(js_name = "promptTokens")]
  pub prompt_tokens: u32,
  #[napi(js_name = "completionTokens")]
  pub completion_tokens: u32,
  #[napi(js_name = "totalTokens")]
  pub total_tokens: u32,
  #[napi(js_name = "promptTokensDetails")]
  pub prompt_tokens_details: Option<PromptTokensDetails>,
}

#[napi]
pub enum BudgetStatus {
  Normal,
  Warning,
  Exceeded,
  Critical,
}

#[napi(object)]
pub struct TokenUsage {
  #[napi(js_name = "promptTokens")]
  pub prompt_tokens: u32,
  #[napi(js_name = "completionTokens")]
  pub completion_tokens: u32,
  #[napi(js_name = "totalTokens")]
  pub total_tokens: u32,
}

impl From<InnerTokenUsage> for TokenUsage {
  fn from(usage: InnerTokenUsage) -> Self {
    Self {
      prompt_tokens: usage.prompt_tokens,
      completion_tokens: usage.completion_tokens,
      total_tokens: usage.total_tokens,
    }
  }
}

#[napi(object)]
pub struct TokenBudgetReport {
  pub usage: TokenUsage,
  pub status: BudgetStatus,
  #[napi(js_name = "usagePercent")]
  pub usage_percent: f32,
  #[napi(js_name = "remainingTokens")]
  pub remaining_tokens: u32,
}

impl From<InnerTokenBudgetReport> for TokenBudgetReport {
  fn from(report: InnerTokenBudgetReport) -> Self {
    Self {
      usage: report.usage.into(),
      status: match report.status {
        react::token_counter::BudgetStatus::Normal => BudgetStatus::Normal,
        react::token_counter::BudgetStatus::Warning => BudgetStatus::Warning,
        react::token_counter::BudgetStatus::Exceeded => BudgetStatus::Exceeded,
        react::token_counter::BudgetStatus::Critical => BudgetStatus::Critical,
      },
      usage_percent: report.usage_percent,
      remaining_tokens: report.remaining_tokens,
    }
  }
}
