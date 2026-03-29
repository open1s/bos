use react::tool::FnTool;
use react::{Action, Memory, Observation, SimpleExecutor, Tool, ToolRegistry};
use serde_json::{json, Value};

#[test]
fn multi_tool_integration_final() {
    // Minimal deterministic integration test for WeatherAPI only
    let mut registry = ToolRegistry::new();
    registry.tools.insert(
        "WeatherAPI".to_string(),
        Box::new(FnTool {
            name: "WeatherAPI".to_string(),
            f: Box::new(|args: &Value| {
                let city = args
                    .get("city")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                json!({"city": city, "forecast": "sunny"})
            }),
        }),
    );

    let mut memory = Memory {
        history: Vec::new(),
    };
    let mut exec = SimpleExecutor::new();

    let a = Action::ToolCall {
        name: "WeatherAPI".to_string(),
        args: json!({"city": "NY"}),
    };
    let o = exec.execute(&a, &mut memory, &mut registry);

    // Basic assertion on output
    assert!(o.text.contains("forecast"));
}
