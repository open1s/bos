wit_bindgen::generate!({ path: "wit", world: "bsh-extension" });

struct Spin;

impl exports::bsh::extension::extension::Guest for Spin {
    fn initialize(_host_version: String, _config: String) -> Result<(), bsh::extension::types::HostError> {
        Ok(())
    }
    fn activate() -> Result<(), bsh::extension::types::HostError> { Ok(()) }
    fn deactivate() -> Result<(), bsh::extension::types::HostError> { Ok(()) }
    fn shutdown() -> Result<(), bsh::extension::types::HostError> { Ok(()) }

    fn tools() -> Vec<bsh::extension::types::ToolDefinition> {
        vec![
            bsh::extension::types::ToolDefinition {
                id: "spin".to_string(),
                description: "Spin forever until the host cancels the call".to_string(),
                input_schema: "{\"type\":\"object\",\"properties\":{}}".to_string(),
            },
            bsh::extension::types::ToolDefinition {
                id: "echo".to_string(),
                description: "Echo arguments; proves the runtime survives a cancel".to_string(),
                input_schema: "{\"type\":\"object\",\"properties\":{}}".to_string(),
            },
        ]
    }

    fn call_tool(id: String, arguments: String, _correlation_id: String) -> Result<bsh::extension::types::ToolResult, bsh::extension::types::HostError> {
        if id == "spin" {
            // Announce that the guest is running, then burn CPU with no host
            // calls. Only the epoch/fuel interruption can stop this loop.
            bsh::extension::host::progress(1, 2);
            loop {
                std::hint::spin_loop();
            }
        }
        Ok(bsh::extension::types::ToolResult { content: arguments, is_error: false })
    }

    fn command(_id: String, _arguments: String) -> Result<String, bsh::extension::types::HostError> {
        Ok("\"command\"".to_string())
    }

    fn panel(_id: String) -> Result<String, bsh::extension::types::HostError> {
        Ok(r#"{"schema_version":"1.0","root":{"type":"text","text":"spin panel"}}"#.to_string())
    }

    fn on_event(_event: bsh::extension::types::Event) {}
}

export!(Spin);
