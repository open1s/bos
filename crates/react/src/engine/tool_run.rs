use super::*;

#[derive(serde::Serialize)]
#[qserde::Archive]
#[rkyv(crate = qserde::rkyv)]
/// Lifecycle event for a tool call published on the bus.
pub struct ToolCallEvent {
    pub call_id: String,
    pub tool: String,
    /// "started" | "completed" | "cancelled" | "failed"
    pub status: String,
    pub timestamp_ms: u64,
}

#[derive(Clone)]
pub struct ToolRunManager {
    running: Arc<DashMap<String, String>>,
    bus: Option<Bus>,
    agent_name: String,
}

impl Default for ToolRunManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRunManager {
    pub fn new() -> Self {
        Self {
            running: Arc::new(DashMap::new()),
            bus: None,
            agent_name: String::new(),
        }
    }

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
                let mut bus = bus;
                let _ = bus.publish(&topic, &payload).await;
            });
        }
    }

    pub fn register(&self, call_id: &str, name: &str) {
        self.running.insert(call_id.to_string(), name.to_string());
        self.publish_event(call_id, name, "started");
    }

    pub fn is_running(&self, call_id: &str) -> bool {
        self.running.contains_key(call_id)
    }

    pub fn cancel(&self, call_id: &str) -> Option<String> {
        let name = self.running.remove(call_id).map(|(_, n)| n);
        if let Some(ref n) = name {
            self.publish_event(call_id, n, "cancelled");
        }
        name
    }

    pub fn complete(&self, call_id: &str) {
        if let Some((_, name)) = self.running.remove(call_id) {
            self.publish_event(call_id, &name, "completed");
        }
    }

    pub fn fail(&self, call_id: &str) {
        if let Some((_, name)) = self.running.remove(call_id) {
            self.publish_event(call_id, &name, "failed");
        }
    }

    pub fn cancel_all_running(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        for entry in self.running.iter() {
            result.push((entry.key().clone(), entry.value().clone()));
        }
        for (id, name) in &result {
            self.running.remove(id);
            self.publish_event(id, name, "cancelled");
        }
        result
    }

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
