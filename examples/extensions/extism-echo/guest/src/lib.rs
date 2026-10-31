use extism_pdk::*;
use serde_json::{json, Value};

#[host_fn]
extern "ExtismHost" {
    fn bsh_host(input: String) -> String;
}

#[plugin_fn]
pub fn bsh_tools(_input: String) -> FnResult<String> {
    Ok(json!([{
        "id": "echo",
        "description": "Echo through Extism",
        "input_schema": {"type": "object", "properties": {"text": {"type": "string"}}},
        "capabilities": []
    }])
    .to_string())
}

#[plugin_fn]
pub fn bsh_call_tool(input: String) -> FnResult<String> {
    let request: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let arguments = request.get("arguments").cloned().unwrap_or(Value::Null);
    let clock = unsafe { bsh_host(json!({"op": "now_millis"}).to_string())? };
    Ok(json!({
        "content": [{"type": "text", "text": arguments.to_string()}],
        "structuredContent": {"host": clock},
        "isError": false
    })
    .to_string())
}

#[plugin_fn]
pub fn bsh_command(input: String) -> FnResult<String> {
    let request: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let id = request.get("id").and_then(Value::as_str).unwrap_or("greet");
    Ok(json!({"id": id, "inserted": "hello from Extism"}).to_string())
}

#[plugin_fn]
pub fn bsh_panel(_input: String) -> FnResult<String> {
    Ok(json!({
        "schema_version": "1.0",
        "revision": 1,
        "root": {
            "type": "column",
            "children": [
                {"type": "heading", "text": "Extism", "level": 2},
                {"type": "text", "text": "This panel comes from the plugin's bsh_panel export."}
            ]
        }
    })
    .to_string())
}

#[plugin_fn]
pub fn bsh_event(_input: String) -> FnResult<String> {
    Ok("{}".to_string())
}
