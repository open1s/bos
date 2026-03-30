use react::tool::FnTool;
use react::{Action, ExecutionOutput, Memory, SimpleExecutor, ToolRegistry};
use serde_json::{json, Value};

#[test]
fn smoke_react_basic_integration() {
    // Build a simple registry with one echo tool
    let mut registry = ToolRegistry::new();
    registry.tools.insert(
        "echo".to_string(),
        Box::new(FnTool {
            name: "echo".to_string(),
            f: Box::new(|args: &Value| json!({"echo": args})),
        }),
    );

    // Prepare memory and executor
    let mut memory = Memory {
        history: Vec::new(),
    };
    let exec = SimpleExecutor::new();

    // Create a tool call Action
    let a = Action::ToolCall {
        name: "echo".to_string(),
        args: json!({"greet": "world"}),
    };

    // Execute and verify some output
    let out: ExecutionOutput = exec.execute(&a, &mut memory, &mut registry);
    assert!(out.text.contains("echo") || !memory.history.is_empty());
}
