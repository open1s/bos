use napi_derive::napi;

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