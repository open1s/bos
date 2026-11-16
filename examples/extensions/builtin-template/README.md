# Built-in Extension Starter

A built-in extension is linked into BSH, but it still has a manifest, declared
capabilities, a lifecycle and the same host contract. Use it only for
first-party code that must ship in the binary; use declarative, process, WASM,
Extism or MCP for everything else.

## The trait

```rust
use std::sync::Arc;
use async_trait::async_trait;
use bsh::extension::runtime::builtin::{BuiltinExtension, BuiltinRuntime};
use bsh::extension::runtime::{ToolDescriptor, empty_schema};
use bsh::extension::types::ExtensionError;
use serde_json::{json, Value};

struct HelloBuiltin;

#[async_trait]
impl BuiltinExtension for HelloBuiltin {
    fn manifest_toml(&self) -> &'static str {
        r#"
schema_version = "1.0"
[extension]
id = "com.example.hello-builtin"
name = "Hello Builtin"
version = "1.0.0"
description = "A first-party built-in starter"
[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"
[runtime]
kind = "builtin"
[capabilities]
required = ["agent.tools.register"]
[contributes]
tools = ["hello"]
"#
    }

    fn tools(&self) -> Vec<ToolDescriptor> {
        vec![ToolDescriptor {
            id: "hello".parse().expect("tool id"),
            description: "Say hello".to_string(),
            input_schema: empty_schema(),
        }]
    }

    fn call_tool(&self, name: &str, _arguments: &Value) -> Result<Value, ExtensionError> {
        match name {
            "hello" => Ok(json!({"message": "hello from a built-in"})),
            other => Err(ExtensionError::NotFound { id: other.to_string() }),
        }
    }
}

fn runtime() -> BuiltinRuntime {
    BuiltinRuntime::new(Arc::new(HelloBuiltin)).expect("built-in manifest")
}
```

Register the runtime with the host the same way the inspector built-in is
registered; the package does not live in the installed store.

## Rules

- The id must be reverse-domain and stable.
- Every tool id must appear in `contributes.tools`.
- Capabilities are checked exactly like an external package.
- No file/network/process access outside the capability API.
- Keep the manifest first-party; a built-in is a trust level, not an exemption.
