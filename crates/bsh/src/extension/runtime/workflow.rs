//! Declarative workflow contributions.
//!
//! A workflow is a bounded sequence of calls to the SAME extension's tools. The
//! wrapper advertises one tool per workflow; its descriptor carries the union of
//! the steps' capabilities, so the host authorizes everything the workflow can
//! reach before the first step runs. The wrapper never reaches another package.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::declarative::RawContribution;
use super::{CallContext, Runtime, ToolDescriptor, ToolResult};
use crate::extension::capability::Capability;
use crate::extension::types::{ExtensionError, RuntimeKind};

/// One step in a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorkflowStep {
    /// Step name, used in the output map and in templates.
    pub(crate) name: String,
    /// The internal tool to call.
    pub(crate) tool: String,
    /// Arguments, with {{input.x}} and {{steps.name.x}} placeholders.
    #[serde(default)]
    pub(crate) arguments: Value,
}

/// One declared workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorkflowFile {
    /// Display description.
    #[serde(default)]
    pub(crate) description: String,
    /// JSON Schema for the workflow input.
    #[serde(default, rename = "input_schema")]
    pub(crate) input_schema: Option<Value>,
    /// Ordered steps.
    #[serde(default)]
    pub(crate) steps: Vec<WorkflowStep>,
}

/// A runtime that adds workflow tools in front of another runtime.
pub(crate) struct WorkflowRuntime {
    inner: Arc<dyn Runtime>,
    workflows: BTreeMap<String, WorkflowFile>,
    tools: Vec<ToolDescriptor>,
}

impl WorkflowRuntime {
    /// Wrap a runtime with the workflows its package declared.
    pub(crate) fn new(
        inner: Arc<dyn Runtime>,
        contributions: &[RawContribution],
        inner_tools: &[ToolDescriptor],
    ) -> Result<Self, ExtensionError> {
        let mut workflows = BTreeMap::new();
        let mut tools = inner_tools.to_vec();
        for contribution in contributions {
            let text = std::fs::read_to_string(&contribution.path).map_err(|source| {
                ExtensionError::Io {
                    path: contribution.path.clone(),
                    source,
                }
            })?;
            let workflow: WorkflowFile =
                serde_json::from_str(&text).map_err(|error| ExtensionError::Json {
                    path: contribution.path.clone(),
                    message: error.to_string(),
                })?;
            let mut capabilities: Vec<Capability> = Vec::new();
            for step in &workflow.steps {
                if let Some(tool) = inner_tools
                    .iter()
                    .find(|tool| tool.id.as_str() == step.tool)
                {
                    for capability in &tool.capabilities {
                        if !capabilities.contains(capability) {
                            capabilities.push(*capability);
                        }
                    }
                }
            }
            tools.push(ToolDescriptor {
                id: contribution.id.clone(),
                description: if workflow.description.is_empty() {
                    format!("Run the {} workflow", contribution.id)
                } else {
                    workflow.description.clone()
                },
                input_schema: workflow
                    .input_schema
                    .clone()
                    .unwrap_or_else(|| json!({"type":"object","properties":{}})),
                capabilities,
            });
            workflows.insert(contribution.id.as_str().to_string(), workflow);
        }
        Ok(Self {
            inner,
            workflows,
            tools,
        })
    }

    /// The combined descriptors.
    pub(crate) fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    async fn execute(
        &self,
        workflow: &WorkflowFile,
        input: Value,
        context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        let mut outputs = serde_json::Map::new();
        for step in &workflow.steps {
            let arguments = resolve(&step.arguments, &input, &outputs);
            let value = self.inner.call_tool(&step.tool, arguments, context).await?;
            outputs.insert(step.name.clone(), value);
        }
        Ok(json!({ "steps": outputs }))
    }
}

fn resolve(template: &Value, input: &Value, steps: &serde_json::Map<String, Value>) -> Value {
    match template {
        Value::String(text) => Value::String(expand(text, input, steps)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve(item, input, steps))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), resolve(value, input, steps)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn expand(text: &str, input: &Value, steps: &serde_json::Map<String, Value>) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        let Some(end) = rest[start + 2..].find("}}") else {
            result.push_str(&rest[start..]);
            return result;
        };
        let path = rest[start + 2..start + 2 + end].trim();
        let replacement = lookup(path, input, steps)
            .map(|value| match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        result.push_str(&replacement);
        rest = &rest[start + 2 + end + 2..];
    }
    result.push_str(rest);
    result
}

fn lookup<'a>(
    path: &str,
    input: &'a Value,
    steps: &'a serde_json::Map<String, Value>,
) -> Option<&'a Value> {
    let mut parts = path.split('.');
    let root = parts.next()?;
    let mut current = match root {
        "input" => input,
        "steps" => steps.get(parts.next()?)?,
        _ => return None,
    };
    for part in parts {
        current = current.get(part)?;
    }
    Some(current)
}

#[async_trait]
impl Runtime for WorkflowRuntime {
    fn kind(&self) -> RuntimeKind {
        self.inner.kind()
    }

    async fn activate(&self) -> Result<(), ExtensionError> {
        self.inner.activate().await
    }

    async fn deactivate(&self) -> Result<(), ExtensionError> {
        self.inner.deactivate().await
    }

    async fn shutdown(&self) -> Result<(), ExtensionError> {
        self.inner.shutdown().await
    }

    async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        Ok(self.tools.clone())
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        if let Some(workflow) = self.workflows.get(name) {
            return self.execute(workflow, arguments, context).await;
        }
        self.inner.call_tool(name, arguments, context).await
    }

    async fn call_tool_result(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<ToolResult, ExtensionError> {
        if let Some(workflow) = self.workflows.get(name) {
            return self
                .execute(workflow, arguments, context)
                .await
                .map(ToolResult::from_value);
        }
        self.inner.call_tool_result(name, arguments, context).await
    }

    async fn command(&self, id: &str, arguments: Value) -> Result<Value, ExtensionError> {
        self.inner.command(id, arguments).await
    }

    async fn panel(&self, id: &str) -> Result<Value, ExtensionError> {
        self.inner.panel(id).await
    }

    async fn pump(&self) -> Result<(), ExtensionError> {
        self.inner.pump().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::types::ContributionId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting(AtomicUsize);

    #[async_trait]
    impl Runtime for Counting {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Declarative
        }
        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(vec![ToolDescriptor {
                id: ContributionId::parse("echo").expect("id"),
                description: "echo".to_string(),
                input_schema: json!({"type":"object"}),
                capabilities: Vec::new(),
            }])
        }
        async fn call_tool(
            &self,
            _name: &str,
            arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(arguments)
        }
    }

    #[test]
    fn templates_resolve_input_and_previous_steps() {
        let input = json!({"name": "bos"});
        let mut steps = serde_json::Map::new();
        steps.insert("first".to_string(), json!({"value": 7}));
        assert_eq!(
            expand("hello {{input.name}} {{steps.first.value}}", &input, &steps),
            "hello bos 7"
        );
    }

    #[tokio::test]
    async fn a_workflow_runs_its_steps_in_order() {
        let scratch = std::env::temp_dir().join(format!("bsh-workflow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).expect("scratch");
        let path = scratch.join("greet.json");
        std::fs::write(
            &path,
            serde_json::to_string(&json!({
                "description": "greet",
                "input_schema": {"type":"object"},
                "steps": [{"name":"echo","tool":"echo","arguments":{"who":"{{input.who}}"}}]
            }))
            .expect("json"),
        )
        .expect("workflow");
        let inner = Arc::new(Counting(AtomicUsize::new(0)));
        let tools = inner.tools().await.expect("tools");
        let runtime = WorkflowRuntime::new(
            inner.clone(),
            &[RawContribution {
                id: ContributionId::parse("greet").expect("id"),
                path,
            }],
            &tools,
        )
        .expect("runtime");
        let context = CallContext::new(
            crate::extension::types::ExtensionId::parse("com.example.workflow").expect("id"),
            "test",
            None,
            5000,
        );
        let result = runtime
            .call_tool("greet", json!({"who":"bos"}), &context)
            .await
            .expect("workflow");
        assert_eq!(result["steps"]["echo"]["who"], "bos");
        assert_eq!(inner.0.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
