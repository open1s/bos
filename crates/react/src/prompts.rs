use serde_json::Value;

pub const DEFAULT_TEMPLATES: &str = r#"Thought: {thought}
Action: {action}
Action Input: {input}
Observation: {observation}
Final Thought: {final_thought}
Final Answer: {final_answer}"#;

pub struct PromptTemplate {
    template: String,
}

impl PromptTemplate {
    pub fn new(template: &str) -> Self {
        Self {
            template: template.to_string(),
        }
    }
    pub fn render(
        &self,
        thought: &str,
        action: &str,
        input: &Value,
        observation: &Value,
        final_thought: &str,
        final_answer: &str,
    ) -> String {
        self.template
            .replace("{thought}", thought)
            .replace("{action}", action)
            .replace("{input}", &input.to_string())
            .replace("{observation}", &observation.to_string())
            .replace("{final_thought}", final_thought)
            .replace("{final_answer}", final_answer)
    }
}

// Simple structured prompt helper for Plan B improvements
// Produces a compact prompt hinting at memory size to guide the LLM's reasoning
pub fn render_structured_prompt(user_input: &str, memory_len: usize) -> String {
    format!(
        "Structured Prompt - Input: {} | Memory items: {}",
        user_input, memory_len
    )
}
