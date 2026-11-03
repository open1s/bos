//! Compiles the SDK without BSH, agent or GPUI in the dependency graph.

use ferris_core::{Contribution, InvokeContext};
use ferris_sdk::prelude::*;

struct Demo;

#[async_trait]
impl Extension for Demo {
    fn manifest(&self) -> ExtensionManifest {
        ManifestBuilder::new("com.example.demo", "Demo", "1.0.0", "SDK smoke extension")
            .runtime("builtin")
            .capability("agent.tools.register")
            .contribution(ContributionSpec::tool("echo"))
            .build()
            .expect("manifest")
    }

    async fn start(&self, _ctx: &ExtensionContext) -> Result<Vec<Contribution>> {
        Ok(vec![
            ContributionSpec::tool("echo").into_contribution(json!({ "description": "Echo" }))
        ])
    }

    async fn invoke(&self, call: &Call, _ctx: &InvokeContext) -> Result<Value> {
        Ok(call.input.clone())
    }
}

#[tokio::main]
async fn main() {
    let manifest = Demo.manifest();
    assert_eq!(manifest.id().as_str(), "com.example.demo");
    assert_eq!(manifest.runtime_kind(), "builtin");
    assert!(manifest.to_toml().contains("[[contribution]]"));
    let harness =
        TestHarness::new(Demo, std::path::PathBuf::from("/tmp/ferris-demo")).expect("harness");
    let contributions = harness.start().await.expect("start");
    assert_eq!(contributions.len(), 1);
    let value = harness
        .invoke("echo", json!({ "ok": true }))
        .await
        .expect("invoke");
    assert_eq!(value, json!({ "ok": true }));
    harness.shutdown().await.expect("shutdown");
}
