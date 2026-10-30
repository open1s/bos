wit_bindgen::generate!({ path: "wit", world: "bsh-extension" });

struct Echo;

impl exports::bsh::extension::extension::Guest for Echo {
    fn initialize(_host_version: String, _config: String) -> Result<(), bsh::extension::types::HostError> {
        Ok(())
    }
    fn activate() -> Result<(), bsh::extension::types::HostError> { Ok(()) }
    fn deactivate() -> Result<(), bsh::extension::types::HostError> { Ok(()) }
    fn shutdown() -> Result<(), bsh::extension::types::HostError> { Ok(()) }

    fn tools() -> Vec<bsh::extension::types::ToolDefinition> {
        vec![bsh::extension::types::ToolDefinition {
            id: "echo".to_string(),
            description: "Echo arguments through the wasm component".to_string(),
            input_schema: "{\"type\":\"object\",\"properties\":{}}".to_string(),
        }]
    }

    fn call_tool(_id: String, arguments: String, _correlation_id: String) -> Result<bsh::extension::types::ToolResult, bsh::extension::types::HostError> {
        Ok(bsh::extension::types::ToolResult { content: arguments, is_error: false })
    }

    fn command(_id: String, _arguments: String) -> Result<String, bsh::extension::types::HostError> {
        Ok("\"command\"".to_string())
    }

    fn panel(_id: String) -> Result<String, bsh::extension::types::HostError> {
        Ok(r#"{"schema_version":"1.0","root":{"type":"text","text":"wasm panel"}}"#.to_string())
    }

    fn on_event(_event: bsh::extension::types::Event) {}
}

export!(Echo);
