use pyo3::prelude::*;

#[pyclass(name = "PromptTokensDetails", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PyPromptTokensDetails {
    #[pyo3(get, set)]
    pub audio_tokens: Option<u32>,
    #[pyo3(get, set)]
    pub cached_tokens: Option<u32>,
}

#[pymethods]
impl PyPromptTokensDetails {
    #[new]
    pub fn new(audio_tokens: Option<u32>, cached_tokens: Option<u32>) -> Self {
        Self {
            audio_tokens,
            cached_tokens,
        }
    }
}

#[pyclass(name = "LlmUsage", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PyLlmUsage {
    #[pyo3(get, set)]
    pub prompt_tokens: u32,
    #[pyo3(get, set)]
    pub completion_tokens: u32,
    #[pyo3(get, set)]
    pub total_tokens: u32,
}

#[pymethods]
impl PyLlmUsage {
    #[new]
    pub fn new(prompt_tokens: u32, completion_tokens: u32, total_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
        }
    }
}

#[pyclass(name = "TokenUsage", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PyTokenUsage {
    #[pyo3(get, set)]
    pub prompt_tokens: u32,
    #[pyo3(get, set)]
    pub completion_tokens: u32,
    #[pyo3(get, set)]
    pub total_tokens: u32,
}

#[pymethods]
impl PyTokenUsage {
    #[new]
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
pub enum PyBudgetStatus {
    Normal,
    Warning,
    Exceeded,
    Critical,
}

#[pyclass(name = "TokenBudgetReport", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PyTokenBudgetReport {
    #[pyo3(get, set)]
    pub prompt_tokens: u32,
    #[pyo3(get, set)]
    pub completion_tokens: u32,
    #[pyo3(get, set)]
    pub total_tokens: u32,
    #[pyo3(get, set)]
    pub status: String,
    #[pyo3(get, set)]
    pub usage_percent: f32,
    #[pyo3(get, set)]
    pub remaining_tokens: u32,
}

#[pymethods]
impl PyTokenBudgetReport {
    #[new]
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