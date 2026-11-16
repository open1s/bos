//! Prompt-template helpers shared with the JavaScript binding.

use std::collections::BTreeMap;

use crate::utils::py_to_json;
use pyo3::prelude::*;

/// Substitute {{name}} placeholders in a prompt template.
///
/// Whitespace inside the braces is ignored. An unknown variable or an unclosed
/// {{ is an error, so a typo is reported instead of shipped literally into a
/// prompt.
#[pyfunction]
pub fn render_template(template: String, variables: &Bound<'_, PyAny>) -> PyResult<String> {
    let value = py_to_json(variables)?;
    let object = value.as_object().ok_or_else(|| {
        pyo3::exceptions::PyTypeError::new_err("template variables must be a mapping of strings")
    })?;
    let mut map = BTreeMap::new();
    for (name, value) in object {
        let value = value.as_str().ok_or_else(|| {
            pyo3::exceptions::PyTypeError::new_err(format!(
                "template variable '{name}' must be a string"
            ))
        })?;
        map.insert(name.clone(), value.to_string());
    }
    let borrowed: BTreeMap<&str, &str> = map
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    react::template::render_template(&template, &borrowed)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))
}

/// Place a recalled-memory block into a system prompt.
///
/// {{memory}} is replaced in place when present; otherwise the block is
/// appended after a blank line. A null or empty block leaves the prompt alone.
#[pyfunction]
#[pyo3(signature = (template, memory = None))]
pub fn with_memory(template: String, memory: Option<String>) -> String {
    react::template::with_memory(&template, memory.as_deref())
}

/// Estimate the token count of text with the agent's own heuristic.
#[pyfunction]
pub fn estimate_tokens(text: String) -> u32 {
    agent::estimate_tokens(&text).min(u32::MAX as usize) as u32
}

/// Register the template helpers on the native module.
pub(crate) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(render_template, module)?)?;
    module.add_function(wrap_pyfunction!(with_memory, module)?)?;
    module.add_function(wrap_pyfunction!(estimate_tokens, module)?)?;
    Ok(())
}
