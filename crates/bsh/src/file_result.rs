//! The LLM-oriented result contract shared by BSH's built-in file tools.
//!
//! A file tool is called by a model, not read by a person, so every result says
//! what it is and whether it is complete. The shared vocabulary is deliberately
//! small so it costs few tokens on every call:
//!
//! - `tool`: the tool that produced the result;
//! - `status`: `ok`, `empty`, `partial` or `error`;
//! - `next`: the exact arguments for the continuation call, absent when none;
//! - `hint`: a one-sentence recovery note, absent when the result speaks for
//!   itself.
//!
//! The tool-specific payload (`content`, `matches`, `entries`, `files`, ...)
//! keeps its existing shape. The contract adds semantics; it does not move the
//! data the tools already return.

use serde_json::Value;

/// The call completed and returned what it found.
pub(crate) const OK: &str = "ok";

/// The call completed and found nothing.
pub(crate) const EMPTY: &str = "empty";

/// Some content was returned, but the result is incomplete.
pub(crate) const PARTIAL: &str = "partial";

/// The call failed, or every item in a batch failed.
pub(crate) const ERROR: &str = "error";

/// The status for a call that returned `empty` content and may be truncated.
pub(crate) fn status(empty: bool, truncated: bool) -> &'static str {
    if truncated {
        PARTIAL
    } else if empty {
        EMPTY
    } else {
        OK
    }
}

/// Add the shared contract fields to one tool result in place.
///
/// `next` is omitted when it is `None`, which is the common case: a result
/// that is complete should not spend tokens saying "no continuation". A tool
/// keeps its own `hint` when it has one; this function does not touch it.
pub(crate) fn finish(value: &mut Value, tool: &str, status: &str, next: Option<Value>) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    object.insert("tool".to_string(), Value::String(tool.to_string()));
    object.insert("status".to_string(), Value::String(status.to_string()));
    if let Some(next) = next {
        object.insert("next".to_string(), next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_separates_complete_empty_and_partial() {
        assert_eq!(status(false, false), OK);
        assert_eq!(status(true, false), EMPTY);
        assert_eq!(status(false, true), PARTIAL);
        assert_eq!(status(true, true), PARTIAL);
    }

    #[test]
    fn finish_adds_the_shared_fields_and_omits_an_absent_continuation() {
        let mut value = json!({ "content": "line" });
        finish(&mut value, "read_file", OK, None);
        assert_eq!(value["tool"], "read_file");
        assert_eq!(value["status"], "ok");
        assert!(value.get("truncated").is_none());
        assert!(value.get("next").is_none());
        assert_eq!(value["content"], "line");
    }

    #[test]
    fn finish_carries_the_exact_continuation_arguments() {
        let mut value = json!({ "content": "window" });
        finish(
            &mut value,
            "read_file",
            PARTIAL,
            Some(json!({ "offset": 13 })),
        );
        assert_eq!(value["status"], "partial");
        assert!(value.get("truncated").is_none());
        assert_eq!(value["next"]["offset"], 13);
    }

    #[test]
    fn finish_keeps_a_tool_specific_hint() {
        let mut value = json!({ "hint": "the file is empty" });
        finish(&mut value, "read_file", EMPTY, None);
        assert_eq!(value["hint"], "the file is empty");
    }
}
