//! Runtime-neutral invocation contract.
//!
//! The manager never knows whether a package is Rust, a declared file tree or a
//! process. It knows a runtime can list its tools and answer a call, and it
//! passes the extension id and a correlation id so every runtime reports the
//! same diagnostics. Concrete runtimes live in this module's children.

pub(crate) mod builtin;
pub(crate) mod declarative;
pub(crate) mod mcp;
pub(crate) mod process;
pub(crate) mod wasm;
pub(crate) mod workflow;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::capability::Capability;
use super::manifest::{validate_relative_path, Manifest};
use super::types::{ContributionId, ExtensionError, ExtensionId, RuntimeKind};

/// The largest tool descriptor file.
pub(crate) const MAX_TOOL_FILE_BYTES: u64 = 256 * 1024;

/// The default tool schema when a file omits one.
pub(crate) fn empty_schema() -> Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

/// One tool a runtime offers, independent of how it is implemented.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolDescriptor {
    /// Contribution id inside the owning extension.
    pub(crate) id: ContributionId,
    /// One-sentence description the model sees.
    pub(crate) description: String,
    /// JSON Schema for the arguments.
    pub(crate) input_schema: Value,
    /// Capabilities a call needs. Empty means only the registration
    /// capability, which was authorized at activation.
    pub(crate) capabilities: Vec<Capability>,
}

/// One part of a tool result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ToolContent {
    /// Plain text.
    Text(String),
    /// An image, base64 encoded with its media type.
    Image {
        /// MIME type such as image/png.
        media_type: String,
        /// Base64 bytes.
        data: String,
        /// Alternative text.
        alt: String,
    },
    /// A file, base64 encoded with its media type.
    File {
        /// Display name.
        name: String,
        /// MIME type.
        media_type: String,
        /// Base64 bytes.
        data: String,
    },
    /// Arbitrary structured JSON.
    Json(Value),
}

/// A rich tool result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolResult {
    /// Ordered content parts.
    pub(crate) content: Vec<ToolContent>,
    /// Machine-readable structured content, when the tool produced one.
    pub(crate) structured: Option<Value>,
    /// Whether the tool reported a failure.
    pub(crate) is_error: bool,
}

impl ToolResult {
    /// A single text result.
    pub(crate) fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text(text.into())],
            structured: None,
            is_error: false,
        }
    }

    /// Wrap a runtime's JSON value, recognizing MCP-shaped content.
    pub(crate) fn from_value(value: Value) -> Self {
        if let Value::Object(object) = &value {
            if let Some(items) = object.get("content").and_then(Value::as_array) {
                let content = items.iter().map(content_from_value).collect();
                return Self {
                    content,
                    structured: object.get("structuredContent").cloned(),
                    is_error: object
                        .get("isError")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                };
            }
        }
        if let Value::String(text) = &value {
            return Self::text(text.clone());
        }
        Self {
            content: vec![ToolContent::Json(value)],
            structured: None,
            is_error: false,
        }
    }

    /// Convert back to the JSON shape the agent transcript stores.
    pub(crate) fn to_value(&self) -> Value {
        if !self.is_error && self.structured.is_none() && self.content.len() == 1 {
            match &self.content[0] {
                ToolContent::Text(text) => return Value::String(text.clone()),
                ToolContent::Json(value) => return value.clone(),
                _ => {}
            }
        }
        let mut object = serde_json::Map::new();
        object.insert(
            "content".to_string(),
            Value::Array(self.content.iter().map(content_to_value).collect()),
        );
        if let Some(structured) = &self.structured {
            object.insert("structuredContent".to_string(), structured.clone());
        }
        if self.is_error {
            object.insert("isError".to_string(), Value::Bool(true));
        }
        Value::Object(object)
    }
}

fn content_from_value(value: &Value) -> ToolContent {
    match value.get("type").and_then(Value::as_str) {
        Some("text") => ToolContent::Text(
            value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        Some("image") => ToolContent::Image {
            media_type: value
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_string(),
            data: value
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            alt: value
                .get("alt")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        Some("resource") => ToolContent::File {
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("resource")
                .to_string(),
            media_type: value
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_string(),
            data: value
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        _ => ToolContent::Json(value.clone()),
    }
}

fn content_to_value(content: &ToolContent) -> Value {
    match content {
        ToolContent::Text(text) => serde_json::json!({"type":"text","text":text}),
        ToolContent::Image {
            media_type,
            data,
            alt,
        } => serde_json::json!({
            "type":"image","mimeType":media_type,"data":data,"alt":alt
        }),
        ToolContent::File {
            name,
            media_type,
            data,
        } => serde_json::json!({
            "type":"resource","name":name,"mimeType":media_type,"data":data
        }),
        ToolContent::Json(value) => value.clone(),
    }
}

/// The per-call context every runtime receives.
#[derive(Debug, Clone)]
pub(crate) struct CallContext {
    /// The owning extension.
    pub(crate) extension: ExtensionId,
    /// The correlation id shared by the request, its events and its logs.
    pub(crate) correlation_id: String,
    /// The workspace open for this call, used for scoped authorization.
    pub(crate) workspace: Option<PathBuf>,
    /// The call's wall-clock ceiling.
    pub(crate) timeout_ms: u64,
}

impl CallContext {
    /// Build a call context.
    pub(crate) fn new(
        extension: ExtensionId,
        correlation_id: impl Into<String>,
        workspace: Option<PathBuf>,
        timeout_ms: u64,
    ) -> Self {
        Self {
            extension,
            correlation_id: correlation_id.into(),
            workspace,
            timeout_ms,
        }
    }
}

/// The runtime seam. Every concrete runtime implements the same five
/// operations and normalizes its own failures into ExtensionError.
#[async_trait]
pub(crate) trait Runtime: Send + Sync {
    /// The runtime kind.
    fn kind(&self) -> RuntimeKind;

    /// Prepare the runtime. A process runtime may defer the first spawn; a
    /// wasm runtime may compile here.
    async fn activate(&self) -> Result<(), ExtensionError> {
        Ok(())
    }

    /// Stop accepting calls and release live resources.
    async fn deactivate(&self) -> Result<(), ExtensionError> {
        Ok(())
    }

    /// Release everything. Called once when the package is unloaded.
    async fn shutdown(&self) -> Result<(), ExtensionError> {
        self.deactivate().await
    }

    /// The tools this runtime offers.
    async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError>;

    /// Invoke one tool by contribution id.
    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<Value, ExtensionError>;

    /// Invoke one tool and preserve rich content parts.
    async fn call_tool_result(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<ToolResult, ExtensionError> {
        self.call_tool(name, arguments, context)
            .await
            .map(ToolResult::from_value)
    }

    /// Execute one command contribution. A runtime without commands refuses.
    async fn command(&self, id: &str, _arguments: Value) -> Result<Value, ExtensionError> {
        Err(ExtensionError::UnsupportedRuntime {
            kind: format!("{} command {id}", self.kind()),
        })
    }

    /// Fetch one panel's current declarative document. A runtime without
    /// panels refuses.
    async fn panel(&self, id: &str) -> Result<Value, ExtensionError> {
        Err(ExtensionError::UnsupportedRuntime {
            kind: format!("{} panel {id}", self.kind()),
        })
    }

    /// Deliver any event the host queued for a subscriber.
    async fn pump(&self) -> Result<(), ExtensionError> {
        Ok(())
    }
}

/// The on-disk tool descriptor every declarative and process package ships for
/// each declared tool.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ToolFile {
    /// Description shown to the model. Defaults to the manifest name.
    #[serde(default)]
    pub(crate) description: Option<String>,
    /// JSON Schema for the arguments.
    #[serde(default, rename = "input_schema")]
    pub(crate) input_schema: Option<Value>,
    /// Capabilities a call needs.
    #[serde(default)]
    pub(crate) capabilities: Vec<Capability>,
    /// The declarative implementation. Process tools ignore it.
    #[serde(default)]
    pub(crate) implementation: Option<Implementation>,
}

/// A pure declarative tool body. Nothing here can touch the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Implementation {
    /// Return the arguments unchanged.
    EchoArguments,
    /// Return a fixed value.
    Constant {
        /// The value.
        value: Value,
    },
    /// Interpolate '{{name}}' placeholders from the arguments into text.
    Template {
        /// The template.
        text: String,
    },
}

impl Implementation {
    /// Run the body with the arguments, bounded by the output ceiling.
    pub(crate) fn run(
        &self,
        arguments: &Value,
        max_output_bytes: u64,
        extension: &ExtensionId,
    ) -> Result<Value, ExtensionError> {
        let value = match self {
            Self::EchoArguments => arguments.clone(),
            Self::Constant { value } => value.clone(),
            Self::Template { text } => {
                let rendered = render_template(text, arguments);
                Value::String(rendered)
            }
        };
        let bytes = serde_json::to_vec(&value)
            .map(|bytes| bytes.len())
            .unwrap_or(0);
        if bytes as u64 > max_output_bytes {
            return Err(ExtensionError::Limit {
                extension: extension.to_string(),
                message: format!("tool result is {bytes} bytes; the ceiling is {max_output_bytes}"),
            });
        }
        Ok(value)
    }
}

/// Replace top-level '{{name}}' placeholders. A missing argument renders as an
/// empty string, which keeps an optional placeholder from failing a tool.
pub(crate) fn render_template(template: &str, arguments: &Value) -> String {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            output.push_str(&rest[start..]);
            return output;
        };
        let key = after[..end].trim();
        let value = arguments.get(key);
        match value {
            Some(Value::String(text)) => output.push_str(text),
            Some(value) => output.push_str(&value.to_string()),
            None => {}
        }
        rest = &after[end + 2..];
    }
    output.push_str(rest);
    output
}

/// Load the tool descriptors for every tool a manifest declares.
///
/// The file name is the full contribution id, so 'git.status' is
/// 'tools/git.status.json'. The full id is used rather than its last segment
/// because two contributions in one package may share a leaf name.
pub(crate) fn load_tool_descriptors(
    root: &Path,
    manifest: &Manifest,
) -> Result<Vec<ToolDescriptor>, ExtensionError> {
    let mut descriptors = Vec::new();
    for id in &manifest.contributes.tools {
        let relative = format!("tools/{}.json", id.as_str());
        let relative =
            validate_relative_path(&relative).map_err(|reason| ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: format!("tool {id}: {reason}"),
            })?;
        let path = root.join(&relative);
        let metadata = std::fs::metadata(&path).map_err(|source| ExtensionError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.len() > MAX_TOOL_FILE_BYTES {
            return Err(ExtensionError::Limit {
                extension: manifest.id().to_string(),
                message: format!(
                    "tool descriptor {} is larger than {MAX_TOOL_FILE_BYTES} bytes",
                    relative.display()
                ),
            });
        }
        let text = std::fs::read_to_string(&path).map_err(|source| ExtensionError::Io {
            path: path.clone(),
            source,
        })?;
        let file: ToolFile = serde_json::from_str(&text).map_err(|error| ExtensionError::Json {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let description = file
            .description
            .filter(|description| !description.trim().is_empty())
            .unwrap_or_else(|| format!("{} ({id})", manifest.extension.name));
        if description.len() > 4096 {
            return Err(ExtensionError::Manifest {
                path,
                message: "tool description must be at most 4096 bytes".to_string(),
            });
        }
        let input_schema = file.input_schema.unwrap_or_else(empty_schema);
        if !input_schema.is_object() {
            return Err(ExtensionError::Manifest {
                path,
                message: "tool input_schema must be a JSON object".to_string(),
            });
        }
        descriptors.push(ToolDescriptor {
            id: id.clone(),
            description,
            input_schema,
            capabilities: file.capabilities,
        });
    }
    Ok(descriptors)
}

/// A runtime wrapper that runs activation once, on first use.
///
/// The host builds an agent synchronously, while activation may need async I/O
/// (a wasm compile, a process handshake). Wrapping every runtime in this cell
/// keeps the build path synchronous and still gives the runtime one activation
/// per process rather than one per call.
pub(crate) struct LazyRuntime {
    inner: Arc<dyn Runtime>,
    activated: tokio::sync::OnceCell<()>,
}

impl LazyRuntime {
    /// Wrap one runtime.
    pub(crate) fn new(inner: Arc<dyn Runtime>) -> Self {
        Self {
            inner,
            activated: tokio::sync::OnceCell::new(),
        }
    }

    async fn ensure(&self) -> Result<(), ExtensionError> {
        self.activated
            .get_or_try_init(|| async { self.inner.activate().await })
            .await
            .map(|_| ())
    }
}

#[async_trait]
impl Runtime for LazyRuntime {
    fn kind(&self) -> RuntimeKind {
        self.inner.kind()
    }

    async fn activate(&self) -> Result<(), ExtensionError> {
        self.ensure().await
    }

    async fn deactivate(&self) -> Result<(), ExtensionError> {
        self.inner.deactivate().await
    }

    async fn shutdown(&self) -> Result<(), ExtensionError> {
        self.inner.shutdown().await
    }

    async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        self.ensure().await?;
        self.inner.tools().await
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        self.ensure().await?;
        self.inner.call_tool(name, arguments, context).await
    }

    async fn call_tool_result(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<ToolResult, ExtensionError> {
        self.ensure().await?;
        self.inner.call_tool_result(name, arguments, context).await
    }

    async fn command(&self, id: &str, arguments: Value) -> Result<Value, ExtensionError> {
        self.ensure().await?;
        self.inner.command(id, arguments).await
    }

    async fn panel(&self, id: &str) -> Result<Value, ExtensionError> {
        self.ensure().await?;
        self.inner.panel(id).await
    }

    async fn pump(&self) -> Result<(), ExtensionError> {
        // Pump must not activate an on-demand package: an idle host with no
        // subscribers should pay nothing for a package nobody called yet.
        self.inner.pump().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let index = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "bsh-ext-runtime-{label}-{}-{index}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join("tools")).expect("scratch");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn manifest() -> Manifest {
        Manifest::parse(
            r#"
schema_version = "1.0"

[extension]
id = "com.example.tools"
name = "Tools"
version = "1.0.0"
description = "Test tools"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[contributes]
tools = ["echo", "greet"]
"#,
            Path::new("extension.toml"),
        )
        .expect("manifest")
    }

    #[test]
    fn descriptors_load_from_the_declared_files() {
        let scratch = Scratch::new("load");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo arguments","input_schema":{"type":"object"}}"#,
        )
        .expect("echo");
        std::fs::write(
            scratch.0.join("tools/greet.json"),
            r#"{"description":"Greet","capabilities":["workspace.read"]}"#,
        )
        .expect("greet");
        let descriptors = load_tool_descriptors(&scratch.0, &manifest()).expect("descriptors");
        assert_eq!(descriptors.len(), 2);
        assert_eq!(descriptors[0].id.as_str(), "echo");
        assert_eq!(descriptors[1].capabilities, vec![Capability::WorkspaceRead]);
    }

    #[test]
    fn a_missing_descriptor_file_is_an_error() {
        let scratch = Scratch::new("missing");
        std::fs::write(scratch.0.join("tools/echo.json"), "{}").expect("echo");
        let error = load_tool_descriptors(&scratch.0, &manifest()).expect_err("missing greet");
        assert!(matches!(error, ExtensionError::Io { .. }), "{error}");
    }

    #[test]
    fn templates_interpolate_top_level_arguments() {
        assert_eq!(
            render_template(
                "hello {{name}} #{{n}}",
                &serde_json::json!({"name":"world","n":2})
            ),
            "hello world #2"
        );
        assert_eq!(render_template("{{missing}}x", &serde_json::json!({})), "x");
        assert_eq!(
            render_template("{{unfinished", &serde_json::json!({})),
            "{{unfinished"
        );
    }

    #[test]
    fn declarative_output_is_bounded() {
        let extension = manifest().id().clone();
        let implementation = Implementation::Template {
            text: "x".repeat(64),
        };
        assert!(implementation
            .run(&serde_json::json!({}), 1024, &extension)
            .is_ok());
        assert!(implementation
            .run(&serde_json::json!({}), 16, &extension)
            .is_err());
    }
}
