//! Minimal SDK extension: a tool that echoes its input.

use ferris_sdk::prelude::*;

struct Echo;

#[async_trait]
impl Extension for Echo {
    fn manifest(&self) -> ExtensionManifest {
        ManifestBuilder::new(
            "com.example.echo",
            "Echo",
            "1.0.0",
            "Echoes the input it receives",
        )
        .author("Ferris")
        .license("MIT")
        .capability("agent.tools.register")
        .contribution(ContributionSpec::tool("echo"))
        .build()
        .expect("manifest")
    }

    async fn start(&self, _ctx: &ExtensionContext) -> Result<Vec<Contribution>> {
        Ok(vec![ContributionSpec::tool("echo").into_contribution(
            json!({
                "description": "Echo the input back",
                "input_schema": { "type": "object" }
            }),
        )])
    }

    async fn invoke(&self, call: &Call, _ctx: &InvokeContext) -> Result<Value> {
        Ok(call.input.clone())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let harness = TestHarness::new(Echo, std::path::PathBuf::from("/tmp/ferris-echo"))?;
    println!("{}", harness.runtime().manifest().to_toml());
    let contributions = harness.start().await?;
    assert_eq!(contributions.len(), 1);
    let value = harness
        .invoke_contribution(kinds::TOOL, "echo", json!({ "text": "hello" }))
        .await?;
    println!("tool result: {value}");
    harness.shutdown().await?;
    Ok(())
}
