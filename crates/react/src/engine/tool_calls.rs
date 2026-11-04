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
            // Events name the tool that really ran; cancellation still
            // resolves to the tool the engine dispatched.
            let label = self.call_identity(name, input).0;
            self.run_manager.register_with_label(&cid, name, &label);
            // If cancel was requested between register and tool execution,
            // skip running the tool entirely.
            if !self.run_manager.is_running(&cid) {
                return Ok(serde_json::json!({
                    "status": "cancelled",
                    "call_id": cid,
                    "elapsed_ms": 0,
                }));
            }
            let result = self.tools.call_with_call_id(name, input, &cid).await;
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

    /// The identity hooks, bus events and the failure guard observe for one
    /// call.
    ///
    /// A proxy tool (see [`crate::tool::registry::Tool::proxy_target`])
    /// forwards a call to another registered tool, and the outside world must
    /// see the tool that actually did the work: policy rules, hook payloads,
    /// telemetry and the repeated-failure guard are all keyed by name. A call
    /// that is not proxied keeps its own name and a copy of its arguments, so a
    /// hook can rewrite them uniformly either way.
    pub(crate) fn call_identity(&self, name: &str, args: &Value) -> (String, Value) {
        match self
            .tools
            .get(name)
            .and_then(|tool| tool.proxy_target(args))
        {
            Some((proxied_name, proxied_args)) => (proxied_name, proxied_args),
            None => (name.to_string(), args.clone()),
        }
    }

    /// Put the arguments a hook was shown back where a proxy reads them.
    ///
    /// A dispatcher carries its target's arguments under `args`; a hook that
    /// rewrote them must have that change reach the tool, not only the payload
    /// the hook saw.
    pub(crate) fn write_proxy_args(args: &mut Value, hooked: Value) {
        if let Some(object) = args.as_object_mut() {
            object.insert("args".to_string(), hooked);
        }
    }
}
