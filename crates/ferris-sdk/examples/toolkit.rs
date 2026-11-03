//! Every manifest and contribution surface the SDK exposes.

use ferris_sdk::prelude::*;

fn main() -> Result<()> {
    let manifest = ManifestBuilder::new(
        "com.example.toolkit",
        "Toolkit",
        "2.1.0",
        "Demonstrates the complete SDK manifest surface",
    )
    .author("Ferris")
    .author("BOS")
    .license("MIT")
    .repository("https://example.com/toolkit")
    .homepage("https://example.com")
    .host(">=3.0.0")
    .extension_api(">=1.0.0")
    .runtime("declarative")
    .runtime_config("entry", toml::Value::String("toolkit.toml".into()))
    .dependency("com.example.base", "^1.0")
    .capability("agent.tools.register")
    .capability("agent.prompts.register")
    .contribution(ContributionSpec::tool("search"))
    .contribution(ContributionSpec::command("refresh"))
    .contribution(ContributionSpec::panel("inspector"))
    .contribution(ContributionSpec::prompt("system"))
    .contribution(ContributionSpec::context("workspace"))
    .contribution(ContributionSpec::skill("review"))
    .contribution(ContributionSpec::workflow("release"))
    .contribution(ContributionSpec::theme("midnight"))
    .contribution(ContributionSpec::preset("fast"))
    .contribution(ContributionSpec::model_provider("local"))
    .contribution(ContributionSpec::agent_provider("reviewer"))
    .contribution(ContributionSpec::language("rust"))
    .contribution(ContributionSpec::integration("github"))
    .contribution(ContributionSpec::subscribe("workspace.opened"))
    .contribution(ContributionSpec::publish("toolkit.ready"))
    .contribution(ContributionSpec::hook("model.request"))
    .provide(ServiceProvideSpec {
        name: "com.example.toolkit.search".into(),
        contract: "com.example.search-api".into(),
        version: Version::parse("1.0.0").expect("version"),
        scope: ServiceScope::Workspace,
        capabilities: Vec::new(),
    })
    .require(ServiceRequireSpec {
        name: "com.example.base.config".into(),
        contract: "com.example.config-api".into(),
        version: Requirement::parse("^1.0").expect("requirement"),
        scope: ServiceScope::Extension,
        optional: true,
    })
    .max_memory_mb(256)
    .max_execution_ms(10_000)
    .max_concurrent_calls(4)
    .max_output_bytes(1_048_576)
    .configuration_schema("schemas/config.schema.json")
    .configuration_defaults(toml::toml! { verbose = false }.into())
    .startup(StartupPolicy::OnWorkspaceOpen)
    .build()?;

    let text = manifest.to_toml();
    println!("{text}");
    assert_eq!(manifest.id().as_str(), "com.example.toolkit");
    assert_eq!(manifest.runtime_kind(), "declarative");
    assert!(text.contains("[[contribution]]"));
    assert!(text.contains("[[services.provides]]"));
    assert!(text.contains("[[services.requires]]"));
    assert!(text.contains("startup = \"on-workspace-open\""));
    Ok(())
}
