use pyo3::prelude::*;
use react::llm::vendor::openaicompatible::{PromptTokensDetails as InnerPromptTokensDetails, Usage as InnerUsage};
use react::token_counter::{TokenUsage as InnerTokenUsage, TokenBudgetReport as InnerTokenBudgetReport, BudgetStatus};

#[pyclass(name = "PromptTokensDetails",skip_from_py_object)]
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

impl From<&InnerUsage> for PyPromptTokensDetails {
    fn from(usage: &InnerUsage) -> Self {
        if let Some(ref details) = usage.prompt_tokens_details {
            Self {
                audio_tokens: details.audio_tokens,
                cached_tokens: details.cached_tokens,
            }
        } else {
            Self {
                audio_tokens: None,
                cached_tokens: None,
            }
        }
    }
}

#[pyclass(name = "LlmUsage")]
#[derive(Clone, Debug)]
pub struct PyLlmUsage {
    #[pyo3(get, set)]
    pub prompt_tokens: u32,
    #[pyo3(get, set)]
    pub completion_tokens: u32,
    #[pyo3(get, set)]
    pub total_tokens: u32,
    #[pyo3(get, set)]
    pub prompt_tokens_details: Option<PyPromptTokensDetails>,
}

#[pymethods]
impl PyLlmUsage {
    #[new]
    pub fn new(
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
    ) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            prompt_tokens_details: None,
        }
    }
}

impl From<&InnerUsage> for PyLlmUsage {
    fn from(usage: &InnerUsage) -> Self {
        let details = if usage.prompt_tokens_details.is_some() {
            Some(PyPromptTokensDetails::from(usage))
        } else {
            None
        };
        
        Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            prompt_tokens_details: details,
        }
    }
}

impl From<PyLlmUsage> for InnerUsage {
    fn from(py_usage: PyLlmUsage) -> Self {
        let details = py_usage.prompt_tokens_details.map(|d| {
            InnerPromptTokensDetails {
                audio_tokens: d.audio_tokens,
                cached_tokens: d.cached_tokens,
            }
        });
        
        InnerUsage {
            prompt_tokens: py_usage.prompt_tokens,
            completion_tokens: py_usage.completion_tokens,
            total_tokens: py_usage.total_tokens,
            prompt_tokens_details: details,
        }
    }
}

#[pyclass(name = "TokenUsage")]
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

impl From<InnerTokenUsage> for PyTokenUsage {
    fn from(usage: InnerTokenUsage) -> Self {
        Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        }
    }
}

#[pyclass(name = "BudgetStatus")]
#[derive(Clone, Debug)]
pub enum PyBudgetStatus {
    Normal,
    Warning,
    Exceeded,
    Critical,
}

impl From<BudgetStatus> for PyBudgetStatus {
    fn from(status: BudgetStatus) -> Self {
        match status {
            BudgetStatus::Normal => PyBudgetStatus::Normal,
            BudgetStatus::Warning => PyBudgetStatus::Warning,
            BudgetStatus::Exceeded => PyBudgetStatus::Exceeded,
            BudgetStatus::Critical => PyBudgetStatus::Critical,
        }
    }
}

#[pyclass(name = "TokenBudgetReport")]
#[derive(Clone, Debug)]
pub struct PyTokenBudgetReport {
    #[pyo3(get, set)]
    pub usage: PyTokenUsage,
    #[pyo3(get, set)]
    pub status: PyBudgetStatus,
    #[pyo3(get, set)]
    pub usage_percent: f32,
    #[pyo3(get, set)]
    pub remaining_tokens: u32,
}

impl From<InnerTokenBudgetReport> for PyTokenBudgetReport {
    fn from(report: InnerTokenBudgetReport) -> Self {
        Self {
            usage: report.usage.into(),
            status: report.status.into(),
            usage_percent: report.usage_percent,
            remaining_tokens: report.remaining_tokens,
        }
    }
}