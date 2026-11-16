use super::*;

#[derive(serde::Serialize)]
#[qserde::Archive]
#[rkyv(crate = qserde::rkyv)]
/// Lifecycle event for a tool call published on the bus.
pub struct ToolCallEvent {
    /// Identifier of the tool call.
    pub call_id: String,
    /// Name of the tool.
    pub tool: String,
    /// "started" | "completed" | "cancelled" | "failed"
    pub status: String,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
}

#[derive(Clone)]
/// Tracks in-flight tool calls and publishes their lifecycle events.
pub struct ToolRunManager {
    running: Arc<DashMap<String, String>>,
    /// The name events report, when it differs from the dispatched tool.
    ///
    /// A proxy (see [`crate::tool::registry::Tool::proxy_target`]) dispatches
    /// as itself and reports the tool it forwarded to, so this map is usually
    /// shorter than `running` - an entry exists only for a call whose two names
    /// differ.
    labels: Arc<DashMap<String, String>>,
    bus: Option<Bus>,
    agent_name: String,
}

impl Default for ToolRunManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRunManager {
    /// Create a manager with no bus.
    pub fn new() -> Self {
        Self {
            running: Arc::new(DashMap::new()),
            labels: Arc::new(DashMap::new()),
            bus: None,
            agent_name: String::new(),
        }
    }

    /// Attach the bus and agent name used for event topics.
    pub fn with_bus(mut self, bus: Bus, agent_name: String) -> Self {
        self.bus = Some(bus);
        self.agent_name = agent_name;
        self
    }

    fn events_topic(&self) -> String {
        format!("agent/{}/tool/events", self.agent_name)
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn publish_event(&self, call_id: &str, tool: &str, status: &str) {
        if let Some(bus) = &self.bus {
            let event = ToolCallEvent {
                call_id: call_id.to_string(),
                tool: tool.to_string(),
                status: status.to_string(),
                timestamp_ms: Self::now_ms(),
            };
            let topic = self.events_topic();
            let bus = bus.clone();
            tokio::spawn(async move {
                let payload = serde_json::to_string(&event).unwrap_or_default();
                let _ = bus.publish(&topic, &payload).await;
            });
        }
    }

    /// The name an event reports for a running call.
    ///
    /// A dispatcher borrows its target's name: the outside world must hear
    /// about the tool that actually ran, while cancellation still has to
    /// resolve to the tool the engine holds.
    pub(crate) fn label_for(&self, call_id: &str) -> Option<String> {
        self.labels
            .get(call_id)
            .map(|label| label.clone())
            .or_else(|| self.running.get(call_id).map(|name| name.clone()))
    }

    /// Mark `call_id` running and publish a started event.
    pub fn register(&self, call_id: &str, name: &str) {
        self.register_with_label(call_id, name, name);
    }

    /// Mark `call_id` running under `dispatch` while events report `label`.
    ///
    /// The two differ for a proxy: the engine dispatches the dispatcher, and
    /// the dispatcher reports the tool it forwarded to.
    pub fn register_with_label(&self, call_id: &str, dispatch: &str, label: &str) {
        self.running
            .insert(call_id.to_string(), dispatch.to_string());
        self.labels.insert(call_id.to_string(), label.to_string());
        self.publish_event(call_id, label, "started");
    }

    /// Whether `call_id` is still running.
    pub fn is_running(&self, call_id: &str) -> bool {
        self.running.contains_key(call_id)
    }

    /// Mark `call_id` cancelled, returning the DISPATCHED tool name.
    ///
    /// The return value is what cancellation needs: the tool the engine holds.
    /// The event reports the label instead, which a proxy fills with the tool
    /// it forwarded to.
    pub fn cancel(&self, call_id: &str) -> Option<String> {
        let dispatch = self.running.remove(call_id).map(|(_, name)| name);
        if let Some(ref name) = dispatch {
            let label = self.settle_label(call_id, name);
            self.publish_event(call_id, &label, "cancelled");
        }
        dispatch
    }

    /// Read a call's event label, then forget it.
    fn settle_label(&self, call_id: &str, fallback: &str) -> String {
        let label = self
            .label_for(call_id)
            .unwrap_or_else(|| fallback.to_string());
        self.labels.remove(call_id);
        label
    }

    /// Mark `call_id` completed.
    pub fn complete(&self, call_id: &str) {
        if let Some((_, name)) = self.running.remove(call_id) {
            let label = self.settle_label(call_id, &name);
            self.publish_event(call_id, &label, "completed");
        }
    }

    /// Mark `call_id` failed.
    pub fn fail(&self, call_id: &str) {
        if let Some((_, name)) = self.running.remove(call_id) {
            let label = self.settle_label(call_id, &name);
            self.publish_event(call_id, &label, "failed");
        }
    }

    /// Cancel every running call and return their ids and DISPATCHED names.
    pub fn cancel_all_running(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        for entry in self.running.iter() {
            result.push((entry.key().clone(), entry.value().clone()));
        }
        for (id, name) in &result {
            self.running.remove(id);
            let label = self.settle_label(id, name);
            self.publish_event(id, &label, "cancelled");
        }
        result
    }

    /// Subscribe to the cancellation topic and cancel matching tools.
    pub fn start_listener(&self, tools: Arc<ToolRegistry>) {
        let bus = match self.bus.as_ref() {
            Some(b) => b.clone(),
            None => return,
        };
        let topic = format!("agent/{}/tool/cancel", self.agent_name);
        let run_mgr = Arc::new(self.clone());
        tokio::spawn(async move {
            let session = bus.session();
            let mut sub = bus::Subscriber::<String>::new(&topic)
                .with_session(session)
                .await
                .expect("failed to subscribe to cancellation topic");
            while let Some(msg) = sub.recv().await {
                let call_id = serde_json::from_str::<serde_json::Value>(&msg)
                    .ok()
                    .and_then(|v| v.get("call_id").and_then(|s| s.as_str()).map(String::from))
                    .unwrap_or(msg);
                if let Some(name) = run_mgr.cancel(&call_id) {
                    if let Some(tool) = tools.get(&name) {
                        tool.cancel(&call_id);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dispatch and its event label are separate facts.
    ///
    /// Cancellation asks the manager which TOOL the engine holds, so it must
    /// answer the dispatcher; a reader watching the bus asks which tool ran, so
    /// the event must answer the tool behind the dispatcher.
    #[test]
    fn an_event_labels_the_tool_a_dispatcher_ran() {
        let manager = ToolRunManager::new();
        manager.register_with_label("call-1", "proxy_tool", "bash");

        assert!(manager.is_running("call-1"));
        assert_eq!(manager.label_for("call-1").as_deref(), Some("bash"));
        assert_eq!(
            manager.cancel("call-1"),
            Some("proxy_tool".to_string()),
            "cancellation resolves the tool the engine holds"
        );
        assert_eq!(manager.cancel("call-1"), None, "a settled call is gone");
    }

    /// A plain call reports its own name and keeps no separate label.
    #[test]
    fn an_ordinary_call_is_its_own_label() {
        let manager = ToolRunManager::new();
        manager.register("call-2", "bash");
        assert_eq!(manager.label_for("call-2").as_deref(), Some("bash"));
        assert_eq!(manager.cancel("call-2"), Some("bash".to_string()));
    }
}
