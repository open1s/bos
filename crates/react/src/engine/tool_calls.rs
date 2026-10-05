use super::*;

impl<A: ReActApp> ReActEngine<A> {
    /// Call tool - no resilience wrapper (only LLM calls need rate limiting)
    pub async fn call_tool(
        &self,
        name: &str,
        input: &mut Value,
        call_id: &str,
    ) -> Result<Value, ReactError> {
        let cancelable = self
            .tools
            .get(name)
            .map(|t| t.is_cancelable())
            .unwrap_or(false);
        if cancelable {
            let cid = call_id.to_string();
            self.run_manager.register(&cid, name);
            // If cancel was requested between register and tool execution,
            // skip running the tool entirely.
            if !self.run_manager.is_running(&cid) {
                return Ok(serde_json::json!({
                    "status": "cancelled",
                    "call_id": cid,
                    "elapsed_ms": 0,
                }));
            }
            let result = self.tools.call(name, input).await;
            if result.is_ok() {
                self.run_manager.complete(&cid);
            } else {
                self.run_manager.fail(&cid);
            }
            result.map_err(|e| ReactError::ToolError(format!("{:?}", e)))
        } else {
            self.tools
                .call(name, input)
                .await
                .map_err(|e| ReactError::ToolError(format!("{:?}", e)))
        }
    }
}
