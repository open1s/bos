//! Prompt templates and system-prompt composition.
//!
//! [`PromptTemplate`] renders `{{name}}` placeholders from a variable map, and
//! [`with_memory`] places a recalled-memory block into a system prompt without
//! losing control of where it appears.

use std::collections::BTreeMap;

use thiserror::Error;

/// The placeholder [`with_memory`] looks for.
pub const MEMORY_PLACEHOLDER: &str = "{{memory}}";

/// A prompt containing `{{name}}` placeholders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    template: String,
}

impl PromptTemplate {
    /// Wrap `template` as a reusable prompt.
    #[must_use]
    pub fn new(template: impl Into<String>) -> Self {
        Self {
            template: template.into(),
        }
    }

    /// The wrapped template text.
    #[must_use]
    pub fn template(&self) -> &str {
        &self.template
    }

    /// Render the placeholders from `variables`.
    ///
    /// See [`render_template`] for the substitution rules.
    pub fn render(&self, variables: &BTreeMap<&str, &str>) -> Result<String, TemplateError> {
        render_template(&self.template, variables)
    }
}

/// Substitute `{{name}}` placeholders in `template`.
///
/// Whitespace inside the braces is ignored, so `{{ name }}` and `{{name}}` are
/// the same. An unknown name or an unclosed `{{` is an error, so a typo is
/// reported instead of being shipped literally into a prompt.
pub fn render_template(
    template: &str,
    variables: &BTreeMap<&str, &str>,
) -> Result<String, TemplateError> {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        rendered.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err(TemplateError::UnclosedPlaceholder);
        };
        let name = after[..end].trim();
        match variables.get(name) {
            Some(value) => rendered.push_str(value),
            None => return Err(TemplateError::MissingVariable(name.to_string())),
        }
        rest = &after[end + 2..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

/// Place a recalled-memory block into a system prompt.
///
/// When `template` contains [`MEMORY_PLACEHOLDER`], the block is substituted in
/// place, so the caller decides where recall appears. Otherwise the block is
/// appended after a blank line, and an empty prompt returns the block alone.
/// A `None` or empty block removes the placeholder and leaves the rest intact.
#[must_use]
pub fn with_memory(template: &str, memory: Option<&str>) -> String {
    let block = memory.filter(|block| !block.is_empty()).unwrap_or("");
    if template.contains(MEMORY_PLACEHOLDER) {
        return collapse_blank_lines(&template.replace(MEMORY_PLACEHOLDER, block));
    }
    if block.is_empty() {
        return template.to_string();
    }
    if template.is_empty() {
        return block.to_string();
    }
    format!("{template}\n\n{block}")
}

/// Trim the text and collapse runs of blank lines to a single blank line.
fn collapse_blank_lines(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut newlines = 0;
    for character in text.chars() {
        if character == '\n' {
            newlines += 1;
            if newlines <= 2 {
                result.push(character);
            }
        } else {
            newlines = 0;
            result.push(character);
        }
    }
    result.trim().to_string()
}

/// Errors raised while rendering a [`PromptTemplate`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TemplateError {
    /// The template referenced a variable that had no value.
    #[error("missing template variable: {0}")]
    MissingVariable(String),
    /// A `{{` was not closed with `}}`.
    #[error("unclosed placeholder in template")]
    UnclosedPlaceholder,
}
