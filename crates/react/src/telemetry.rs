use serde_json::Value;

#[derive(Debug, Clone)]
pub enum TelemetryEvent {
    PromptSent {
        prompt: String,
    },
    ThoughtGenerated {
        thought: String,
    },
    ToolInvocation {
        tool: String,
        input: Value,
        output: Value,
    },
    MemorySnapshot {
        length: usize,
    },
    FinalAnswer {
        answer: String,
    },
}

#[derive(Debug, Clone)]
pub struct Telemetry {
    enabled: bool,
}

impl Telemetry {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
    pub fn emit(&self, event: &TelemetryEvent) {
        if self.enabled {
            log::info!("[telemetry] {:?}", event);
        }
    }
}
