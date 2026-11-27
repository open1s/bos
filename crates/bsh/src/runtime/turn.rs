//! The request text one turn sends.
//!
//! The reader's line is not always the model's line: an `@file` token needs the
//! resolution guidance, and a plan-mode turn needs the plan-first directive.
//! The composition lives here because BOTH interfaces build it - the window's
//! runner and the Host executor - and a second copy would be a second answer to
//! what the model was asked.

use crate::prompts::PLAN_DIRECTIVE;

/// Everything one request carries in front of the reader's own line.
///
/// The reference does NOT inject goal state into an ordinary model request: a
/// goal mutation is a UI and durable-log fact, and the only goal text a model
/// sees is the round prompt plus any attachment message the command submitted.
/// So this adds only the reference guidance (how to resolve an @token) and plan
/// mode (what it may do), each when it applies; an ordinary turn's input stays
/// exactly the reader's text.
pub(crate) fn turn_text(text: &str, plan_mode: bool) -> String {
    let mut out = String::new();
    if crate::reference::has_references(text) {
        out.push_str(crate::reference::FILE_REFERENCE_PROMPT);
        out.push_str("\n\n");
    }
    if plan_mode {
        out.push_str(PLAN_DIRECTIVE);
        out.push_str("\n\n");
    }
    out.push_str(text);
    out
}

#[cfg(test)]
mod tests {
    use super::turn_text;

    /// A plain turn is the reader's text, byte for byte; plan mode prepends the
    /// directive, and a reference prepends the resolution guidance.
    #[test]
    fn a_turn_carries_only_the_guidance_it_needs() {
        assert_eq!(turn_text("hello", false), "hello");
        let planned = turn_text("hello", true);
        assert!(planned.starts_with(crate::prompts::PLAN_DIRECTIVE));
        assert!(planned.ends_with("hello"));
        let referenced = turn_text("look at @src/main.rs#L3-L9", false);
        assert!(referenced.starts_with(crate::reference::FILE_REFERENCE_PROMPT));
        assert!(referenced.ends_with("look at @src/main.rs#L3-L9"));
    }
}
