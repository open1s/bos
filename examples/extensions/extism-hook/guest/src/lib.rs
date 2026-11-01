//! An Extism extension that consumes BSH observe hooks and counts them.
//!
//! The host calls `bsh_hook` with `{"point": "...", "batch": [...]}`; this
//! plugin keeps one counter per point in its own storage namespace and exposes
//! them through a tool and a panel. It never sees a body: the hook payload is
//! bounded metadata.

use extism_pdk::*;
use serde_json::{json, Value};

#[host_fn]
extern "ExtismHost" {
    fn bsh_host(input: String) -> String;
}

fn host(request: Value) -> Option<Value> {
    let raw = unsafe { bsh_host(request.to_string()) }.ok()?;
    let reply: Value = serde_json::from_str(&raw).ok()?;
    if reply.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    Some(reply.get("value").cloned().unwrap_or(Value::Null))
}

fn read_counts() -> Value {
    host(json!({"op": "storage_get", "key": "counts"}))
        .and_then(|value| value.get("text").cloned())
        .and_then(|text| {
            text.as_str()
                .and_then(|text| serde_json::from_str(text).ok())
        })
        .unwrap_or_else(|| json!({}))
}

fn write_counts(counts: &Value) -> bool {
    host(json!({
        "op": "storage_set",
        "key": "counts",
        "data": counts.to_string(),
    }))
    .is_some()
}

#[plugin_fn]
pub fn bsh_tools(_input: String) -> FnResult<String> {
    Ok(json!([{
        "id": "stats",
        "description": "Observe-hook counters kept by this plugin",
        "input_schema": {"type": "object", "properties": {}},
        "capabilities": []
    }])
    .to_string())
}

#[plugin_fn]
pub fn bsh_call_tool(_input: String) -> FnResult<String> {
    let counts = read_counts();
    Ok(json!({
        "content": [{"type": "text", "text": counts.to_string()}],
        "structuredContent": counts,
        "isError": false
    })
    .to_string())
}

#[plugin_fn]
pub fn bsh_hook(input: String) -> FnResult<String> {
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let point = payload
        .get("point")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let count = payload
        .get("batch")
        .and_then(Value::as_array)
        .map(|batch| batch.len() as u64)
        .unwrap_or(1);
    let mut counts = read_counts();
    let Some(object) = counts.as_object_mut() else {
        return Ok(json!({"action": "continue"}).to_string());
    };
    let current = object.get(&point).and_then(Value::as_u64).unwrap_or(0);
    object.insert(point, json!(current.saturating_add(count)));
    if !write_counts(&counts) {
        return Err(Error::msg("storage_set failed").into());
    }
    Ok(json!({"action": "continue"}).to_string())
}

#[plugin_fn]
pub fn bsh_panel(_input: String) -> FnResult<String> {
    let counts = read_counts();
    let mut children = vec![json!({"type": "heading", "text": "Observe hooks", "level": 2})];
    if let Some(object) = counts.as_object() {
        for (point, count) in object {
            children.push(json!({
                "type": "text",
                "text": format!("{point}: {count}"),
            }));
        }
    }
    if children.len() == 1 {
        children.push(json!({"type": "text", "text": "No hook batch observed yet."}));
    }
    Ok(json!({
        "schema_version": "1.0",
        "revision": 1,
        "root": {"type": "column", "children": children}
    })
    .to_string())
}

#[plugin_fn]
pub fn bsh_event(_input: String) -> FnResult<String> {
    Ok("{}".to_string())
}
