# Ferris SDK

The author-facing crate is ferris-sdk. It depends only on ferris-core; it does
not depend on BSH, GPUI, or the agent crate.

## A minimal extension

~~~rust
use ferris_sdk::prelude::*;

struct Echo;

#[async_trait]
impl Extension for Echo {
    fn manifest(&self) -> ExtensionManifest {
        ManifestBuilder::new("com.example.echo", "Echo", "1.0.0", "Echo input")
            .capability("agent.tools.register")
            .contribution(ContributionSpec::tool("echo"))
            .build()
            .expect("manifest")
    }

    async fn start(&self, _ctx: &ExtensionContext) -> Result<Vec<ferris_core::Contribution>> {
        Ok(vec![ContributionSpec::tool("echo")
            .into_contribution(json!({ "description": "Echo" }))])
    }

    async fn invoke(&self, call: &Call, _ctx: &ferris_core::InvokeContext) -> Result<Value> {
        Ok(call.input.clone())
    }
}
~~~

ManifestBuilder renders a schema 2.0 TOML document and validates it through
ManifestV2::parse. ContributionSpec provides typed constructors for the
standard kinds:

- tool
- command
- panel
- prompt
- context
- skill
- workflow
- theme
- preset
- model_provider
- agent_provider
- language
- integration
- subscribe / publish
- hook

## Runtime adapter

~~~rust
let harness = TestHarness::new(Echo, std::path::PathBuf::from("/tmp/echo"))?;
let contributions = harness.start().await?;
let value = harness.invoke("echo", json!({ "text": "hi" })).await?;
harness.shutdown().await?;
~~~

ExtensionRuntime is the production adapter; TestHarness is the small
in-process convenience wrapper used by SDK tests and ferris-smoke.

## Error helpers

ferris_sdk::error exposes runtime, contribution, invalid_input and
with_details, all returning ferris_core::ExtError.

## Design rules

- No macro DSL. The manifest is the single author entry point.
- The runtime kind is derived from the manifest; authors do not pass a second
  kind string.
- Manifest validation happens before an extension reaches a host.
- Host capability names remain strings because the SDK is host-neutral.
