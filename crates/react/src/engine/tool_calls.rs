use super::*;

impl<A: ReActApp> ReActEngine<A> {
    /// How many extra attempts a read-only tool gets after a transient failure.
    const TRANSIENT_TOOL_RETRIES: usize = 1;

    /// Failure text a retry can plausibly clear. Anything else is the
    /// environment or the request refusing, and a retry would only repeat it.
    const TRANSIENT_FAILURES: &'static [&'static str] = &[
        "timed out",
        "timeout",
        "connection",
        "temporarily",
        "429",
        "502",
        "503",
        "504",
        "reset",
        "broken pipe",
        "try again",
        "network error",
    ];

    /// Whether a failed call may be retried without risking a side effect.
    ///
    /// Two conditions, both conservative: the failure names a transient cause,
    /// and the tool DECLARES read-only side effects. An undeclared tool, or one
    /// declaring write/process/network/spend/auth, keeps the single attempt it
    /// has always had: a retried edit is a second edit.
    fn retryable_tool_failure(&self, name: &str, error: &ReactError) -> bool {
        if Self::TRANSIENT_TOOL_RETRIES == 0 {
            return false;
        }
        let text = error.to_string().to_lowercase();
        if !Self::TRANSIENT_FAILURES
            .iter()
            .any(|needle| text.contains(needle))
        {
            return false;
        }
        self.tools.metadata(name).is_some_and(|metadata| {
            !metadata.side_effects.is_empty()
                && metadata.side_effects.iter().all(|effect| effect == "read")
        })
    }

    /// Run a tool, retrying a declared read-only tool once after a transient
    /// failure.
    ///
    /// A broken environment cannot be made to allow the work, but a timeout or
    /// a reset connection is often gone on the next attempt. The retry is
    /// deliberately narrow so it can never duplicate a side effect, and the
    /// attempt is logged so a host can see it.
    pub async fn run_tool_resilient(
        &self,
        name: &str,
        input: &mut Value,
        call_id: &str,
    ) -> Result<Value, ReactError> {
        let result = self.run_tool(name, input, call_id).await;
        if let Err(error) = &result {
            if self.retryable_tool_failure(name, error) {
                log::warn!("retrying read-only tool {name} after a transient failure: {error}");
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                return self.run_tool(name, input, call_id).await;
            }
        }
        result
    }

    /// Call tool - no resilience wrapper (only LLM calls need rate limiting)
    pub async fn run_tool(
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
