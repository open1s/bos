//! Reading a stored tool result through the engine's result envelope.
//!
//! The engine renders every result as `{status, summary, data, ...}`
//! (see `react::tool::result`), so a host that wants the tool's own object must
//! unwrap `data`. Sessions written before the envelope hold that object at the
//! top level, and both must read the same way: the row is old, the fact is not.

use serde_json::Value;

/// The envelope's own fields; a payload carrying all three at the top level is
/// the envelope, not a tool object that happens to use those words.
fn is_envelope(object: &serde_json::Map<String, Value>) -> bool {
    object.contains_key("status") && object.contains_key("summary") && object.contains_key("data")
}

/// The tool's payload from one stored result.
///
/// Returns None when the text is not JSON at all. A legacy row (no envelope)
/// returns the whole value, so callers never need to know which format they
/// were handed.
pub(crate) fn payload(output: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(output).ok()?;
    Some(match value {
        Value::Object(ref object) if is_envelope(object) => {
            value.get("data").cloned().unwrap_or(Value::Null)
        }
        other => other,
    })
}

/// The envelope's one-sentence summary of a stored result.
///
/// A row written before the envelope has no summary; the caller falls back to
/// the payload's own first line.
pub(crate) fn summary(output: &str) -> Option<String> {
    let value: Value = serde_json::from_str(output).ok()?;
    let object = value.as_object()?;
    if !is_envelope(object) {
        return None;
    }
    object
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_enveloped_result_reads_as_its_payload() {
        let envelope = json!({
            "status": "success",
            "summary": "sed completed",
            "data": {"path": "a.txt", "outcome": "changed", "added": 1, "deleted": 1}
        })
        .to_string();
        let payload = payload(&envelope).expect("valid JSON");
        assert_eq!(payload["path"], "a.txt");
        assert_eq!(payload["outcome"], "changed");
    }

    #[test]
    fn a_legacy_row_reads_as_itself() {
        let legacy = json!({"path": "a.txt", "outcome": "changed", "status": "ok"}).to_string();
        let payload = payload(&legacy).expect("valid JSON");
        assert_eq!(payload["path"], "a.txt", "no data key, no unwrap");
    }

    #[test]
    fn a_failure_envelope_has_a_null_payload() {
        let envelope = json!({
            "status": "error",
            "summary": "sed failed",
            "data": null,
            "error": {"code": "invalid_input", "message": "bad script"}
        })
        .to_string();
        assert!(payload(&envelope).expect("valid JSON").is_null());
    }

    #[test]
    fn a_non_json_output_has_no_payload() {
        assert!(payload("plain command output").is_none());
    }

    #[test]
    fn a_summary_is_read_only_from_an_envelope() {
        let envelope = json!({
            "status": "success",
            "summary": "read_file completed",
            "data": {"content": "x"}
        })
        .to_string();
        assert_eq!(summary(&envelope).as_deref(), Some("read_file completed"));
        assert!(summary(r#"{"content":"x","summary":"user data"}"#).is_none());
        assert!(summary("not json").is_none());
    }
}
