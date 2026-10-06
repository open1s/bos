//! Tests for the async-closure tool adapter.

use agent::prelude::*;
use agent::tools::{AsyncFunctionTool, AsyncTool, ToolError};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn async_function_tool_runs_a_borrowing_closure() {
    let tool = AsyncFunctionTool::new(
        "double",
        "Double a number",
        json!({"type": "object", "properties": {"n": {"type": "number"}}, "required": ["n"]}),
        |args| {
            Box::pin(async move {
                let n = args["n"]
                    .as_f64()
                    .ok_or_else(|| ToolError::Failed("n required".to_string()))?;
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                Ok(json!(n * 2.0))
            })
        },
    );

    assert_eq!(tool.name(), "double");
    assert_eq!(tool.description(), "Double a number");
    assert_eq!(tool.json_schema()["required"], json!(["n"]));
    assert!(!tool.is_skill());
    assert_eq!(tool.category(), "general");
    assert_eq!(tool.run(&json!({"n": 21.0})).await.unwrap(), json!(42.0));
}

#[tokio::test]
async fn async_function_tool_registers_and_runs_through_the_registry() {
    let tool = AsyncFunctionTool::new(
        "now",
        "Return a fixed value",
        json!({"type": "object"}),
        |_args| Box::pin(async { Ok(json!("ok")) }),
    );
    let mut registry = ToolRegistry::new();
    registry.register_async(Arc::new(tool)).unwrap();

    assert_eq!(registry.async_tool_names(), vec!["now".to_string()]);
    let stored = registry.get_async("now").expect("registered");
    assert_eq!(stored.run(&json!({})).await.unwrap(), json!("ok"));

    let duplicate = AsyncFunctionTool::new("now", "Duplicate", json!({}), |_args| {
        Box::pin(async { Ok(json!(null)) })
    });
    assert!(registry.register_async(Arc::new(duplicate)).is_err());
}

#[tokio::test]
async fn async_function_tool_skill_flag_and_category() {
    let tool = AsyncFunctionTool::skill("s", "d", json!({}), |_args| {
        Box::pin(async { Ok(json!(null)) })
    })
    .with_category("custom");

    assert!(tool.is_skill());
    assert_eq!(tool.category(), "custom");
}
