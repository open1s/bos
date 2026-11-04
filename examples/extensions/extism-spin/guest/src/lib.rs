use extism_pdk::*;
use serde_json::{json, Value};

#[plugin_fn]
pub fn bsh_tools(_input: String) -> FnResult<String> {
    Ok(json!([
        {
            "id": "spin",
            "description": "Spin forever until the host cancels the call",
            "input_schema": {"type": "object", "properties": {}},
            "capabilities": []
        },
        {
            "id": "echo",
            "description": "Echo; proves the plugin survives a cancellation",
            "input_schema": {"type": "object", "properties": {}},
            "capabilities": []
        }
    ])
    .to_string())
}

#[plugin_fn]
pub fn bsh_call_tool(input: String) -> FnResult<String> {
    let request: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default();
    if name == "spin" {
        loop {
            std::hint::spin_loop();
        }
    }
    let arguments = request.get("arguments").cloned().unwrap_or(Value::Null);
    Ok(json!({
        "content": [{"type": "text", "text": arguments.to_string()}],
        "isError": false
    })
    .to_string())
}

#[plugin_fn]
pub fn bsh_command(_input: String) -> FnResult<String> {
    Ok(json!({"id": "noop"}).to_string())
}

#[plugin_fn]
pub fn bsh_panel(_input: String) -> FnResult<String> {
    Ok(json!({"schema_version": "1.0", "root": {"type": "text", "text": "spin"}}).to_string())
}

#[plugin_fn]
pub fn bsh_event(_input: String) -> FnResult<String> {
    Ok("{}".to_string())
}
