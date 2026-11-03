//! Compiles the SDK without BSH, agent or GPUI in the dependency graph.

use bos_ext_core::{Contribution, ExtError, InvokeContext};
use bos_ext_sdk::{Call, ContributionBuilder, Extension, ExtensionContext};
use serde_json::Value;

struct Demo;

#[async_trait::async_trait]
impl Extension for Demo {
    fn manifest(&self) -> &'static str {
        r#"
schema_version = "2.0"
[extension]
id = "com.example.demo"
version = "1.0.0"
"#
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
    let contribution = ContributionBuilder::new("demo.echo", "echo")
        .payload(serde_json::json!({ "kind": "demo" }))
        .build();
    assert_eq!(contribution.kind, "demo.echo");
    assert_eq!(contribution.id, "echo");
    let _ = Demo;
}
