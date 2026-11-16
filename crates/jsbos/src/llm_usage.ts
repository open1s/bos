// LlmUsage - Token usage from LLM responses
// This interface mirrors the Usage struct from react::llm::vendor

export interface LlmUsage {
  /** Number of tokens in the prompt */
  promptTokens: number;
  /** Number of tokens in the completion */
  completionTokens: number;
  /** Total tokens used */
  totalTokens: number;
  /** Optional breakdown of prompt tokens */
  promptTokensDetails?: PromptTokensDetails;
}

export interface PromptTokensDetails {
  /** Audio tokens included in the prompt */
  audioTokens?: number;
  /** Cached tokens (if using cache) */
  cachedTokens?: number;
}