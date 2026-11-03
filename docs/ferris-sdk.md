# Ferris SDK

ferris-sdk is the sole author-facing entry point for Ferris. It re-exports the
core types an extension needs, so an extension crate depends on ferris-sdk and
does not import ferris-core directly. It does not depend on BSH, GPUI, or the
agent crate.

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

    async fn start(&self, _ctx: &ExtensionContext) -> Result<Vec<Contribution>> {
        Ok(vec![ContributionSpec::tool("echo")
            .into_contribution(json!({ "description": "Echo" }))])
    }

    async fn invoke(&self, call: &Call, _ctx: &InvokeContext) -> Result<Value> {
        Ok(call.input.clone())
    }
}
~~~

## Manifest surface

ManifestBuilder covers the complete 2.0 manifest:

- identity: id, name, version, description, authors, license, repository,
  homepage
- compatibility: host, extension_api
- runtime: kind and opaque runtime config
- dependencies
- capabilities
- services: provide and require
- resources: memory, execution, concurrency, output bytes
- configuration: schema path and defaults
- contributions
- startup policy

~~~rust
let manifest = ManifestBuilder::new(
    "com.example.toolkit", "Toolkit", "2.1.0", "Toolkit",
)
.author("Ferris")
.license("MIT")
.host(">=3.0.0")
.extension_api(">=1.0.0")
.runtime("declarative")
.runtime_config("entry", toml::Value::String("toolkit.toml".into()))
.dependency("com.example.base", "^1.0")
.capability("agent.tools.register")
.contribution(ContributionSpec::tool("search"))
.max_memory_mb(256)
.max_execution_ms(10_000)
.max_concurrent_calls(4)
.max_output_bytes(1_048_576)
.configuration_schema("schemas/config.schema.json")
.configuration_defaults(toml::toml! { verbose = false }.into())
.startup(StartupPolicy::OnWorkspaceOpen)
.build()?;
~~~

## Contributions

ContributionSpec provides typed constructors for the standard kinds:

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

Each constructor returns one contribution entry in the manifest. At runtime,
convert the same spec into a runtime contribution:

~~~rust
ContributionSpec::tool("search")
    .into_contribution(json!({ "description": "Search" }))
~~~

## Services

~~~rust
.provide(ServiceProvideSpec {
    name: "com.example.search".into(),
    contract: "com.example.search-api".into(),
    version: Version::parse("1.0.0")?,
    scope: ServiceScope::Workspace,
    capabilities: Vec::new(),
})
.require(ServiceRequireSpec {
    name: "com.example.config".into(),
    contract: "com.example.config-api".into(),
    version: Requirement::parse("^1.0")?,
    scope: ServiceScope::Extension,
    optional: true,
})
~~~

## Runtime adapter and testing

ExtensionRuntime is the production adapter. TestHarness is the small in-process
wrapper used by SDK tests and examples.

~~~rust
let harness = TestHarness::new(Echo, std::path::PathBuf::from("/tmp/echo"))?;
let contributions = harness.start().await?;
let tool = harness
    .invoke_contribution(kinds::TOOL, "echo", json!({ "text": "hi" }))
    .await?;
harness.shutdown().await?;
~~~

invoke is shorthand for invoke_contribution with kind tool.

## Error helpers

ferris_sdk::error exposes runtime, contribution, invalid_input and with_details,
all returning ExtError.

## Examples

- crates/ferris-sdk/examples/echo.rs: minimal tool extension and harness.
- crates/ferris-sdk/examples/toolkit.rs: every manifest field, all contribution
  kinds, services, resources, configuration and startup.

Run with:

~~~bash
cargo run -p ferris-sdk --example echo
cargo run -p ferris-sdk --example toolkit
~~~

## Design rules

- No macro DSL. The manifest is the single author entry point.
- The runtime kind is derived from the manifest; authors do not pass a second
  kind string.
- Manifest validation happens before an extension reaches a host.
- Host capability names remain strings because the SDK is host-neutral.
- ferris-core types are re-exported through ferris-sdk and its prelude.
