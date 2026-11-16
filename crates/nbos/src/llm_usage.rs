use pyo3::prelude::*;

#[pyclass(name = "PromptTokensDetails", skip_from_py_object)]
#[derive(Clone, Debug)]
/// Breakdown of the prompt portion of an LLM call.
pub struct PyPromptTokensDetails {
    /// Audio tokens in the prompt.
    #[pyo3(get, set)]
    pub audio_tokens: Option<u32>,
    /// Cached tokens in the prompt.
    #[pyo3(get, set)]
    pub cached_tokens: Option<u32>,
}

#[pymethods]
impl PyPromptTokensDetails {
    #[new]
    /// Create prompt token details.
    pub fn new(audio_tokens: Option<u32>, cached_tokens: Option<u32>) -> Self {
        Self {
            audio_tokens,
            cached_tokens,
        }
    }
}

#[pyclass(name = "LlmUsage", skip_from_py_object)]
#[derive(Debug)]
/// Token usage reported by an LLM provider.
pub struct PyLlmUsage {
    /// Prompt tokens.
    #[pyo3(get, set)]
    pub prompt_tokens: u32,
    /// Completion tokens.
    #[pyo3(get, set)]
    pub completion_tokens: u32,
    /// Total tokens.
    #[pyo3(get, set)]
    pub total_tokens: u32,
    /// Optional prompt token breakdown.
    #[pyo3(get, set)]
    pub prompt_tokens_details: Option<Py<PyPromptTokensDetails>>,
}

#[pymethods]
impl PyLlmUsage {
    #[new]
    #[pyo3(signature = (prompt_tokens, completion_tokens, total_tokens, prompt_tokens_details = None))]
    /// Create a usage record.
    pub fn new(
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
        prompt_tokens_details: Option<Py<PyPromptTokensDetails>>,
    ) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            prompt_tokens_details,
        }
    }
}

#[pyclass(name = "TokenUsage", skip_from_py_object)]
#[derive(Clone, Debug)]
/// Token usage for a single agent run.
pub struct PyTokenUsage {
    /// Prompt tokens.
    #[pyo3(get, set)]
    pub prompt_tokens: u32,
    /// Completion tokens.
    #[pyo3(get, set)]
    pub completion_tokens: u32,
    /// Total tokens.
    #[pyo3(get, set)]
    pub total_tokens: u32,
}

#[pymethods]
impl PyTokenUsage {
    #[new]
    /// Create a token usage record.
    pub fn new(prompt_tokens: u32, completion_tokens: u32, total_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
        }
    }
}

#[pyclass(name = "BudgetStatus", skip_from_py_object)]
#[derive(Clone, Debug)]
/// Status of a token budget.
pub enum PyBudgetStatus {
    /// Within budget.
    Normal,
    /// Nearing the budget.
    Warning,
    /// Over the budget.
    Exceeded,
    /// Critically over the budget.
    Critical,
}

#[pyclass(name = "TokenBudgetReport", skip_from_py_object)]
#[derive(Clone, Debug)]
/// A token budget report.
pub struct PyTokenBudgetReport {
    /// Prompt tokens used.
    #[pyo3(get, set)]
    pub prompt_tokens: u32,
    /// Completion tokens used.
    #[pyo3(get, set)]
    pub completion_tokens: u32,
    /// Total tokens used.
    #[pyo3(get, set)]
    pub total_tokens: u32,
    /// Budget status label.
    #[pyo3(get, set)]
    pub status: String,
    /// Fraction of the budget used.
    #[pyo3(get, set)]
    pub usage_percent: f32,
    /// Tokens left in the budget.
    #[pyo3(get, set)]
    pub remaining_tokens: u32,
}

#[pymethods]
impl PyTokenBudgetReport {
    #[new]
    /// Create a budget report.
    pub fn new(
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
        status: String,
        usage_percent: f32,
        remaining_tokens: u32,
    ) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            status,
            usage_percent,
            remaining_tokens,
        }
    }
}
