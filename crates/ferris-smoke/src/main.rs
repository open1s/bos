//! Compiles the SDK without BSH, agent or GPUI in the dependency graph.

use ferris_core::manifest_v2::ManifestV2;
use ferris_core::{Contribution, ExtError, InvokeContext};
use ferris_sdk::{Call, ContributionBuilder, Extension, ExtensionContext};
use serde_json::Value;

const MANIFEST: &str = r#"
schema_version = "2.0"
[extension]
id = "com.example.demo"
name = "Demo"
version = "1.0.0"
description = "SDK smoke extension"
[compatibility]
host = ">=3.0.0"
extension_api = ">=1.0.0"
[runtime]
kind = "builtin"
[[contribution]]
kind = "demo.echo"
id = "echo"
"#;

struct Demo;

#[async_trait::async_trait]
impl Extension for Demo {
    fn manifest(&self) -> &'static str {
        MANIFEST
    }

    async fn start(&self, _ctx: &ExtensionContext) -> Result<Vec<Contribution>, ExtError> {
        Ok(vec![ContributionBuilder::new("demo.echo", "echo")
            .payload(serde_json::json!({}))
            .build()])
    }

    async fn invoke(&self, _call: &Call, _ctx: &InvokeContext) -> Result<Value, ExtError> {
        Ok(serde_json::json!({ "ok": true }))
    }
}

fn main() {
    let manifest = ManifestV2::parse(Demo.manifest(), std::path::Path::new("extension.toml"))
        .expect("manifest");
    assert_eq!(manifest.extension.id.as_str(), "com.example.demo");
    assert_eq!(manifest.runtime.kind, "builtin");
    assert_eq!(manifest.contributions.len(), 1);
    let contribution = ContributionBuilder::new("demo.echo", "echo")
        .payload(serde_json::json!({ "kind": "demo" }))
        .build();
    assert_eq!(contribution.kind, "demo.echo");
    assert_eq!(contribution.id, "echo");
    let _ = Demo;
}
