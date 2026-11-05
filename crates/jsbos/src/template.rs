//! Prompt-template helpers shared with the Python binding.

use std::collections::BTreeMap;

use napi::bindgen_prelude::*;
use napi_derive::napi;

fn variables_from_json(variables: serde_json::Value) -> Result<BTreeMap<String, String>> {
  let object = variables.as_object().ok_or_else(|| {
    Error::new(
      napi::Status::InvalidArg,
      "template variables must be a plain object of strings",
    )
  })?;
  let mut map = BTreeMap::new();
  for (name, value) in object {
    let value = value.as_str().ok_or_else(|| {
      Error::new(
        napi::Status::InvalidArg,
        format!("template variable '{name}' must be a string"),
      )
    })?;
    map.insert(name.clone(), value.to_string());
  }
  Ok(map)
}

#[napi(js_name = "renderTemplate")]
/// Substitute `{{name}}` placeholders in `template`.
///
/// Whitespace inside the braces is ignored. An unknown variable or an
/// unclosed `{{` is an error, so a typo is reported instead of shipped
/// literally into a prompt.
pub fn render_template(template: String, variables: serde_json::Value) -> Result<String> {
  let variables = variables_from_json(variables)?;
  let borrowed: BTreeMap<&str, &str> = variables
    .iter()
    .map(|(name, value)| (name.as_str(), value.as_str()))
    .collect();
  react::template::render_template(&template, &borrowed)
    .map_err(|error| Error::new(napi::Status::InvalidArg, error.to_string()))
}

#[napi(js_name = "withMemory")]
/// Place a recalled-memory block into a system prompt.
///
/// `{{memory}}` is replaced in place when present; otherwise the block is
/// appended after a blank line. A null or empty block leaves the prompt alone.
pub fn with_memory(template: String, memory: Option<String>) -> String {
  react::template::with_memory(&template, memory.as_deref())
}

#[napi(js_name = "estimateTokens")]
/// Estimate the token count of `text` with the agent's own heuristic.
pub fn estimate_tokens(text: String) -> u32 {
  agent::estimate_tokens(&text).min(u32::MAX as usize) as u32
}
